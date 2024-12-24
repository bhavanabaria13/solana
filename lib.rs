use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use std::collections::HashMap;

declare_id!("EkfuLxBR1w3uHCwV1YfsNHZFkDzEmQsjZYvtUQh9iK51"); // Replace with actual program ID

#[program]
pub mod fee_distributor {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let fee_vault = &mut ctx.accounts.fee_vault;
        fee_vault.liquidity_wallet = ctx.accounts.liquidity_wallet.key();
        fee_vault.reward_wallet = ctx.accounts.reward_wallet.key();
        fee_vault.charity_wallet = ctx.accounts.charity_wallet.key();
        fee_vault.marketing_wallet = ctx.accounts.marketing_wallet.key();
        fee_vault.buyback_wallet = ctx.accounts.buyback_wallet.key();

        fee_vault.liquidity_percentage = 25;
        fee_vault.reward_percentage = 25;
        fee_vault.charity_percentage = 10;
        fee_vault.marketing_percentage = 25;
        fee_vault.buyback_percentage = 15;

        fee_vault.authority = ctx.accounts.authority.key();
        fee_vault.owner = ctx.accounts.owner.key();
        fee_vault.supported_tokens = Vec::new();
        Ok(())
    }

    pub fn add_supported_token(ctx: Context<AddSupportedToken>, mint: Pubkey) -> Result<()> {
        let fee_vault = &mut ctx.accounts.fee_vault;
        if !fee_vault.supported_tokens.contains(&mint) {
            fee_vault.supported_tokens.push(mint);
        }
        Ok(())
    }

    pub fn remove_supported_token(ctx: Context<RemoveSupportedToken>, mint: Pubkey) -> Result<()> {
        let fee_vault = &mut ctx.accounts.fee_vault;
        if let Some(pos) = fee_vault.supported_tokens.iter().position(|x| *x == mint) {
            fee_vault.supported_tokens.remove(pos);
        }
        Ok(())
    }

    pub fn distribute_spl_token_fees(ctx: Context<DistributeTokenFees>) -> Result<()> {
        let fee_vault = &ctx.accounts.fee_vault;
        let total_amount = ctx.accounts.fee_token_account.amount;

        require!(
            fee_vault
                .supported_tokens
                .contains(&ctx.accounts.fee_token_account.mint),
            FeeDistributorError::UnsupportedToken
        );

        // Calculate amounts based on percentages
        let liquidity_amount = (total_amount * fee_vault.liquidity_percentage as u64) / 100;
        let reward_amount = (total_amount * fee_vault.reward_percentage as u64) / 100;
        let charity_amount = (total_amount * fee_vault.charity_percentage as u64) / 100;
        let marketing_amount = (total_amount * fee_vault.marketing_percentage as u64) / 100;
        let buyback_amount = (total_amount * fee_vault.buyback_percentage as u64) / 100;

        // Transfer to each wallet
        let transfers = [
            (liquidity_amount, &ctx.accounts.liquidity_token_account),
            (reward_amount, &ctx.accounts.reward_token_account),
            (charity_amount, &ctx.accounts.charity_token_account),
            (marketing_amount, &ctx.accounts.marketing_token_account),
            (buyback_amount, &ctx.accounts.buyback_token_account),
        ];

        for (amount, destination) in transfers.iter() {
            if *amount > 0 {
                token::transfer(
                    CpiContext::new(
                        ctx.accounts.token_program.to_account_info(),
                        Transfer {
                            from: ctx.accounts.fee_token_account.to_account_info(),
                            to: destination.to_account_info(),
                            authority: ctx.accounts.authority.to_account_info(),
                        },
                    ),
                    *amount,
                )?;
            }
        }

        Ok(())
    }

    pub fn distribute_sol_fees(ctx: Context<DistributeSolFees>) -> Result<()> {
        let fee_vault = &ctx.accounts.fee_vault;
        let total_amount = ctx.accounts.fee_vault_sol.lamports();

        // Calculate amounts based on percentages
        let liquidity_amount = (total_amount * fee_vault.liquidity_percentage as u64) / 100;
        let reward_amount = (total_amount * fee_vault.reward_percentage as u64) / 100;
        let charity_amount = (total_amount * fee_vault.charity_percentage as u64) / 100;
        let marketing_amount = (total_amount * fee_vault.marketing_percentage as u64) / 100;
        let buyback_amount = (total_amount * fee_vault.buyback_percentage as u64) / 100;

        // Transfer SOL to each wallet
        let transfers = [
            (liquidity_amount, &ctx.accounts.liquidity_wallet),
            (reward_amount, &ctx.accounts.reward_wallet),
            (charity_amount, &ctx.accounts.charity_wallet),
            (marketing_amount, &ctx.accounts.marketing_wallet),
            (buyback_amount, &ctx.accounts.buyback_wallet),
        ];

        for (amount, destination) in transfers.iter() {
            if *amount > 0 {
                **ctx.accounts.fee_vault_sol.try_borrow_mut_lamports()? -= *amount;
                **destination.try_borrow_mut_lamports()? += *amount;
            }
        }

        Ok(())
    }

    // Keep the update_percentages function from before
    pub fn update_percentages(
        ctx: Context<UpdatePercentages>,
        liquidity_percentage: u8,
        reward_percentage: u8,
        charity_percentage: u8,
        marketing_percentage: u8,
        buyback_percentage: u8,
    ) -> Result<()> {
        require!(
            liquidity_percentage
                + reward_percentage
                + charity_percentage
                + marketing_percentage
                + buyback_percentage
                == 100,
            FeeDistributorError::InvalidPercentages
        );

        let fee_vault = &mut ctx.accounts.fee_vault;
        fee_vault.liquidity_percentage = liquidity_percentage;
        fee_vault.reward_percentage = reward_percentage;
        fee_vault.charity_percentage = charity_percentage;
        fee_vault.marketing_percentage = marketing_percentage;
        fee_vault.buyback_percentage = buyback_percentage;

        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = authority, space = FeeVault::LEN)]
    pub fee_vault: Account<'info, FeeVault>,
    pub liquidity_wallet: AccountInfo<'info>,
    pub reward_wallet: AccountInfo<'info>,
    pub charity_wallet: AccountInfo<'info>,
    pub marketing_wallet: AccountInfo<'info>,
    pub buyback_wallet: AccountInfo<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub owner: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AddSupportedToken<'info> {
    #[account(
        mut,
        has_one = owner,
        constraint = owner.key() == fee_vault.owner @ FeeDistributorError::UnauthorizedOwner
    )]
    pub fee_vault: Account<'info, FeeVault>,
    pub owner: Signer<'info>,
}

#[derive(Accounts)]
pub struct RemoveSupportedToken<'info> {
    #[account(
        mut,
        has_one = owner,
        constraint = owner.key() == fee_vault.owner @ FeeDistributorError::UnauthorizedOwner
    )]
    pub fee_vault: Account<'info, FeeVault>,
    pub owner: Signer<'info>,
}

#[derive(Accounts)]
pub struct DistributeTokenFees<'info> {
    #[account(has_one = authority)]
    pub fee_vault: Account<'info, FeeVault>,
    #[account(mut)]
    pub fee_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub liquidity_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub reward_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub charity_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub marketing_token_account: Account<'info, TokenAccount>,
    #[account(mut)]
    pub buyback_token_account: Account<'info, TokenAccount>,
    pub authority: Signer<'info>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct DistributeSolFees<'info> {
    #[account(has_one = authority)]
    pub fee_vault: Account<'info, FeeVault>,
    /// CHECK: This is the PDA that holds SOL fees
    #[account(
        mut,
        seeds = [b"fee_vault_sol", fee_vault.key().as_ref()],
        bump
    )]
    pub fee_vault_sol: AccountInfo<'info>,
    /// CHECK: Verified through constraint
    #[account(
        mut,
        constraint = liquidity_wallet.key() == fee_vault.liquidity_wallet
    )]
    pub liquidity_wallet: AccountInfo<'info>,
    /// CHECK: Verified through constraint
    #[account(
        mut,
        constraint = reward_wallet.key() == fee_vault.reward_wallet
    )]
    pub reward_wallet: AccountInfo<'info>,
    /// CHECK: Verified through constraint
    #[account(
        mut,
        constraint = charity_wallet.key() == fee_vault.charity_wallet
    )]
    pub charity_wallet: AccountInfo<'info>,
    /// CHECK: Verified through constraint
    #[account(
        mut,
        constraint = marketing_wallet.key() == fee_vault.marketing_wallet
    )]
    pub marketing_wallet: AccountInfo<'info>,
    /// CHECK: Verified through constraint
    #[account(
        mut,
        constraint = buyback_wallet.key() == fee_vault.buyback_wallet
    )]
    pub buyback_wallet: AccountInfo<'info>,
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdatePercentages<'info> {
    #[account(
        mut,
        has_one = owner,
        constraint = owner.key() == fee_vault.owner @ FeeDistributorError::UnauthorizedOwner
    )]
    pub fee_vault: Account<'info, FeeVault>,
    pub owner: Signer<'info>,
}

#[account]
pub struct FeeVault {
    pub liquidity_wallet: Pubkey,
    pub reward_wallet: Pubkey,
    pub charity_wallet: Pubkey,
    pub marketing_wallet: Pubkey,
    pub buyback_wallet: Pubkey,
    pub liquidity_percentage: u8,
    pub reward_percentage: u8,
    pub charity_percentage: u8,
    pub marketing_percentage: u8,
    pub buyback_percentage: u8,
    pub authority: Pubkey,
    pub owner: Pubkey,
    pub supported_tokens: Vec<Pubkey>,
}

impl FeeVault {
    pub const LEN: usize = 32 * 7 + // 7 Pubkeys
        1 * 5 + // 5 u8s for percentages
        4 + (32 * 50) + // Vec with max 50 supported tokens
        8; // discriminator
}

#[error_code]
pub enum FeeDistributorError {
    #[msg("Sum of percentages must equal 100")]
    InvalidPercentages,
    #[msg("Only the owner can perform this action")]
    UnauthorizedOwner,
    #[msg("Token is not supported")]
    UnsupportedToken,
}
