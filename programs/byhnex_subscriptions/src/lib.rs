#![allow(unexpected_cfgs)]

use anchor_lang::prelude::*;
use anchor_spl::associated_token::get_associated_token_address;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

declare_id!("7fBNUdxMbSvtwhk7mvk3zF5b7nPbtsTmMe6CjSFVYcG");

const MAX_BPS: u16 = 10_000;
const PLATFORM_FEE_BPS: u16 = 500;
const MAX_SUBSCRIPTION_AMOUNT: u64 = 1_000_000_000_000;

#[program]
pub mod eroticweb3_subscriptions {
    use super::*;

    pub fn initialize_config(
        ctx: Context<InitializeConfig>,
        treasury: Pubkey,
        usdc_mint: Pubkey,
    ) -> Result<()> {
        require!(PLATFORM_FEE_BPS <= MAX_BPS, SubscriptionError::InvalidFeeBps);

        let config = &mut ctx.accounts.config;
        config.authority = ctx.accounts.authority.key();
        config.treasury = treasury;
        config.usdc_mint = usdc_mint;
        config.platform_fee_bps = PLATFORM_FEE_BPS;
        config.bump = ctx.bumps.config;
        Ok(())
    }

    pub fn update_config(
        ctx: Context<UpdateConfig>,
        treasury: Pubkey,
        usdc_mint: Pubkey,
    ) -> Result<()> {
        require!(PLATFORM_FEE_BPS <= MAX_BPS, SubscriptionError::InvalidFeeBps);

        let config = &mut ctx.accounts.config;
        config.treasury = treasury;
        config.usdc_mint = usdc_mint;
        config.platform_fee_bps = PLATFORM_FEE_BPS;
        Ok(())
    }

    pub fn pay_subscription(
        ctx: Context<PaySubscription>,
        subscription_id: u64,
        amount: u64,
    ) -> Result<()> {
        require!(subscription_id > 0, SubscriptionError::InvalidSubscriptionId);
        require!(amount > 0, SubscriptionError::InvalidAmount);
        require!(
            amount <= MAX_SUBSCRIPTION_AMOUNT,
            SubscriptionError::AmountTooLarge
        );
        require_keys_neq!(
            ctx.accounts.subscriber.key(),
            ctx.accounts.creator.key(),
            SubscriptionError::InvalidCreator
        );

        let subscriber_key = ctx.accounts.subscriber.key();
        let creator_key = ctx.accounts.creator.key();
        let usdc_mint_key = ctx.accounts.usdc_mint.key();
        let config = &ctx.accounts.config;

        require_keys_eq!(
            usdc_mint_key,
            config.usdc_mint,
            SubscriptionError::InvalidUsdcMint
        );
        require!(
            validate_ata(
                &ctx.accounts.subscriber_usdc_ata,
                &get_associated_token_address(&subscriber_key, &usdc_mint_key),
                &subscriber_key,
                &usdc_mint_key,
            ),
            SubscriptionError::InvalidSubscriberAta
        );
        require!(
            validate_ata(
                &ctx.accounts.creator_usdc_ata,
                &get_associated_token_address(&creator_key, &usdc_mint_key),
                &creator_key,
                &usdc_mint_key,
            ),
            SubscriptionError::InvalidCreatorAta
        );
        require!(
            validate_ata(
                &ctx.accounts.platform_fee_account,
                &get_associated_token_address(&config.treasury, &usdc_mint_key),
                &config.treasury,
                &usdc_mint_key,
            ),
            SubscriptionError::InvalidPlatformFeeAccount
        );
        require!(
            ctx.accounts.subscriber_usdc_ata.amount >= amount,
            SubscriptionError::InsufficientSubscriberFunds
        );

        let fee_amount = calculate_fee(amount, config.platform_fee_bps)?;
        let creator_amount = amount
            .checked_sub(fee_amount)
            .ok_or(SubscriptionError::MathError)?;

        if fee_amount > 0 {
            transfer_tokens(
                ctx.accounts.token_program.to_account_info(),
                ctx.accounts.subscriber_usdc_ata.to_account_info(),
                ctx.accounts.platform_fee_account.to_account_info(),
                ctx.accounts.subscriber.to_account_info(),
                fee_amount,
            )?;
        }

        if creator_amount > 0 {
            transfer_tokens(
                ctx.accounts.token_program.to_account_info(),
                ctx.accounts.subscriber_usdc_ata.to_account_info(),
                ctx.accounts.creator_usdc_ata.to_account_info(),
                ctx.accounts.subscriber.to_account_info(),
                creator_amount,
            )?;
        }

        emit!(SubscriptionPaid {
            subscriber: subscriber_key,
            creator: creator_key,
            subscription_id,
            amount,
            fee: fee_amount,
            creator_amount,
        });
        Ok(())
    }
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    #[account(
        init,
        payer = authority,
        space = 8 + Config::SPACE,
        seeds = [b"config"],
        bump
    )]
    pub config: Account<'info, Config>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [b"config"],
        bump = config.bump,
        has_one = authority
    )]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct PaySubscription<'info> {
    #[account(mut)]
    pub subscriber: Signer<'info>,

    /// CHECK: creator wallet provided by the client and validated against ATA ownership.
    pub creator: UncheckedAccount<'info>,

    #[account(
        seeds = [b"config"],
        bump = config.bump
    )]
    pub config: Account<'info, Config>,

    #[account(mut)]
    pub subscriber_usdc_ata: Account<'info, TokenAccount>,

    #[account(mut)]
    pub creator_usdc_ata: Account<'info, TokenAccount>,

    #[account(mut)]
    pub platform_fee_account: Account<'info, TokenAccount>,

    pub usdc_mint: Account<'info, Mint>,
    pub token_program: Program<'info, Token>,
}

#[account]
pub struct Config {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub usdc_mint: Pubkey,
    pub platform_fee_bps: u16,
    pub bump: u8,
}

impl Config {
    pub const SPACE: usize = 32 * 3 + 2 + 1;
}

#[event]
pub struct SubscriptionPaid {
    pub subscriber: Pubkey,
    pub creator: Pubkey,
    pub subscription_id: u64,
    pub amount: u64,
    pub fee: u64,
    pub creator_amount: u64,
}

fn calculate_fee(amount: u64, fee_bps: u16) -> Result<u64> {
    let fee = (amount as u128)
        .checked_mul(fee_bps as u128)
        .ok_or(SubscriptionError::MathError)?
        / MAX_BPS as u128;
    Ok(fee as u64)
}

fn validate_ata(
    ata: &Account<TokenAccount>,
    expected_address: &Pubkey,
    owner: &Pubkey,
    mint: &Pubkey,
) -> bool {
    ata.key() == *expected_address && ata.owner == *owner && ata.mint == *mint
}

fn transfer_tokens<'info>(
    token_program: AccountInfo<'info>,
    from: AccountInfo<'info>,
    to: AccountInfo<'info>,
    authority: AccountInfo<'info>,
    amount: u64,
) -> Result<()> {
    token::transfer(
        CpiContext::new(
            token_program,
            Transfer {
                from,
                to,
                authority,
            },
        ),
        amount,
    )
}

#[error_code]
pub enum SubscriptionError {
    #[msg("ID abonnement invalide")]
    InvalidSubscriptionId,
    #[msg("Montant invalide")]
    InvalidAmount,
    #[msg("Montant trop eleve")]
    AmountTooLarge,
    #[msg("Createur invalide")]
    InvalidCreator,
    #[msg("Fee invalide")]
    InvalidFeeBps,
    #[msg("Erreur math")]
    MathError,
    #[msg("Subscriber ATA invalide")]
    InvalidSubscriberAta,
    #[msg("Creator ATA invalide")]
    InvalidCreatorAta,
    #[msg("Compte fee plateforme invalide")]
    InvalidPlatformFeeAccount,
    #[msg("USDC mint invalide")]
    InvalidUsdcMint,
    #[msg("Fonds subscriber insuffisants")]
    InsufficientSubscriberFunds,
}
