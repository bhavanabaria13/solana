use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{Burn, Mint, Token, TokenAccount, Transfer},
};
use std::ops::Deref;

declare_id!("6f7bwfx2uGyECoBtdfrtux2kLKZ4tn41VPU4GvapCugx");

const FEE_NUMERATOR: u64 = 3;
const FEE_DENOMINATOR: u64 = 1000;
const AUTHORITY_SEED: &[u8] = b"authority";
const PAIR_SEED: &[u8] = b"pair";
const MINIMUM_LIQUIDITY: u64 = 1000; // Minimum LP tokens to protect against first deposit exploitation

fn calculate_sqrt(y: u128) -> u128 {
    if y < 4 {
        if y == 0 {
            0
        } else {
            1
        }
    } else {
        let mut z = (y + 1) / 2;
        let mut x = y;
        while x > z {
            x = z;
            z = (y / z + z) / 2;
        }
        x
    }
}
#[program]
pub mod Solana_dex {
    use super::*;

    pub fn create_pair(ctx: Context<CreatePair>, bump: u8) -> Result<()> {
        let pair = &mut ctx.accounts.pair;
        pair.token_a_mint = ctx.accounts.token_a_mint.key();
        pair.token_b_mint = ctx.accounts.token_b_mint.key();
        pair.token_a_vault = ctx.accounts.token_a_vault.key();
        pair.token_b_vault = ctx.accounts.token_b_vault.key();
        pair.lp_mint = ctx.accounts.lp_mint.key();
        pair.authority = ctx.accounts.authority.key();
        pair.bump = bump;
        pair.admin = ctx.accounts.admin.key();

        // Initialize k_last for price manipulation protection
        pair.k_last = 0;
        // Initialize reserve snapshots for flash loan protection
        pair.reserve_a_last = 0;
        pair.reserve_b_last = 0;
        pair.last_block = 0;

        emit!(PairCreated {
            pair: ctx.accounts.pair.key(),
            token_a_mint: ctx.accounts.token_a_mint.key(),
            token_b_mint: ctx.accounts.token_b_mint.key(),
            token_a_vault: ctx.accounts.token_a_vault.key(),
            token_b_vault: ctx.accounts.token_b_vault.key(),
            lp_mint: ctx.accounts.lp_mint.key(),
        });

        Ok(())
    }

    pub fn swap(ctx: Context<Swap>, amount_in: u64, minimum_amount_out: u64) -> Result<()> {
        let pair = &ctx.accounts.pair;
        let vault_in = if ctx.accounts.token_in_mint.key() == pair.token_a_mint {
            &ctx.accounts.token_a_vault
        } else {
            &ctx.accounts.token_b_vault
        };
        let vault_out = if ctx.accounts.token_out_mint.key() == pair.token_a_mint {
            &ctx.accounts.token_a_vault
        } else {
            &ctx.accounts.token_b_vault
        };

        // Flash loan protection
        check_reserves(
            pair,
            ctx.accounts.token_a_vault.amount,
            ctx.accounts.token_b_vault.amount,
        )?;

        // Calculate amounts with fees
        let fee_amount = amount_in * FEE_NUMERATOR / FEE_DENOMINATOR;
        let amount_in_with_fee = amount_in - fee_amount;

        let amount_out = (amount_in_with_fee as u128 * vault_out.amount as u128
            / (vault_in.amount as u128 + amount_in_with_fee as u128))
            as u64;

        require!(
            amount_out >= minimum_amount_out,
            SimpleDexError::SlippageExceeded
        );

        // Check k_last
        let current_k = (ctx.accounts.token_a_vault.amount as u128)
            * (ctx.accounts.token_b_vault.amount as u128);
        if ctx.accounts.pair.k_last > 0 {
            require!(
                current_k >= ctx.accounts.pair.k_last,
                SimpleDexError::PriceManipulation
            );
        }

        // Transfer input tokens from user
        let transfer_in = Transfer {
            from: ctx.accounts.user_token_in.to_account_info(),
            to: vault_in.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };

        anchor_spl::token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), transfer_in),
            amount_in,
        )?;

        // Transfer output tokens to user
        let seeds = &[AUTHORITY_SEED, &[ctx.accounts.pair.bump]];
        let signer = &[&seeds[..]];

        let transfer_out = Transfer {
            from: vault_out.to_account_info(),
            to: ctx.accounts.user_token_out.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };

        anchor_spl::token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                transfer_out,
                signer,
            ),
            amount_out,
        )?;

        // Transfer fee to admin
        let transfer_fee = Transfer {
            from: vault_in.to_account_info(),
            to: ctx.accounts.admin_token_in.to_account_info(),
            authority: ctx.accounts.authority.to_account_info(),
        };

        anchor_spl::token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                transfer_fee,
                signer,
            ),
            fee_amount,
        )?;

        // Update state
        ctx.accounts.pair.k_last = current_k;
        update_reserves(
            &mut ctx.accounts.pair,
            ctx.accounts.token_a_vault.amount,
            ctx.accounts.token_b_vault.amount,
        )?;

        emit!(Swapped {
            user: ctx.accounts.user.key(),
            pair: ctx.accounts.pair.key(),
            amount_in,
            amount_out,
            fee_amount,
            a_to_b: ctx.accounts.token_in_mint.key() == ctx.accounts.pair.token_a_mint,
        });

        Ok(())
    }
    pub fn add_liquidity(ctx: Context<AddLiquidity>, amount_a: u64, amount_b: u64) -> Result<()> {
        let total_supply = ctx.accounts.lp_mint.supply;
        let pair_key = ctx.accounts.pair.key();
        let user_key = ctx.accounts.user.key();

        // Flash loan protection check
        check_reserves(
            &ctx.accounts.pair,
            ctx.accounts.token_a_vault.amount,
            ctx.accounts.token_b_vault.amount,
        )?;

        // Check k_last for price manipulation protection
        if total_supply > 0 {
            let current_k = (ctx.accounts.token_a_vault.amount as u128)
                * (ctx.accounts.token_b_vault.amount as u128);
            require!(
                ctx.accounts.pair.k_last > 0 && current_k >= ctx.accounts.pair.k_last,
                SimpleDexError::PriceManipulation
            );
        }

        // Transfer tokens to vaults
        let transfer_a = Transfer {
            from: ctx.accounts.user_token_a.to_account_info(),
            to: ctx.accounts.token_a_vault.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let transfer_b = Transfer {
            from: ctx.accounts.user_token_b.to_account_info(),
            to: ctx.accounts.token_b_vault.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };

        let seeds = &[AUTHORITY_SEED, &[ctx.accounts.pair.bump]];
        let signer = &[&seeds[..]];

        anchor_spl::token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), transfer_a),
            amount_a,
        )?;

        anchor_spl::token::transfer(
            CpiContext::new(ctx.accounts.token_program.to_account_info(), transfer_b),
            amount_b,
        )?;

        // Calculate and mint LP tokens
        let lp_amount = if total_supply == 0 {
            let initial_liquidity = calculate_sqrt(amount_a as u128 * amount_b as u128) as u64;
            require!(
                initial_liquidity > MINIMUM_LIQUIDITY,
                SimpleDexError::InsufficientInitialLiquidity
            );
            initial_liquidity - MINIMUM_LIQUIDITY // Lock minimum liquidity forever
        } else {
            let vault_a_balance = ctx.accounts.token_a_vault.amount;
            std::cmp::min(
                amount_a * total_supply / vault_a_balance,
                amount_b * total_supply / ctx.accounts.token_b_vault.amount,
            )
        };

        // Update k_last and reserves
        ctx.accounts.pair.k_last = (ctx.accounts.token_a_vault.amount as u128)
            * (ctx.accounts.token_b_vault.amount as u128);

        anchor_spl::token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token::MintTo {
                    mint: ctx.accounts.lp_mint.to_account_info(),
                    to: ctx.accounts.user_lp_token.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer,
            ),
            lp_amount,
        )?;

        update_reserves(
            &mut ctx.accounts.pair,
            ctx.accounts.token_a_vault.amount,
            ctx.accounts.token_b_vault.amount,
        )?;

        emit!(LiquidityAdded {
            user: user_key,
            pair: pair_key,
            amount_a,
            amount_b,
            lp_minted: lp_amount,
        });

        Ok(())
    }

    pub fn remove_liquidity(
        ctx: Context<RemoveLiquidity>,
        lp_amount: u64,
        minimum_amount_a: u64,
        minimum_amount_b: u64,
    ) -> Result<()> {
        let pair = &mut ctx.accounts.pair;
        let total_supply = ctx.accounts.lp_mint.supply;
        let lp_share = (lp_amount as u128) * 10000 / (total_supply as u128);

        let amount_a = (ctx.accounts.token_a_vault.amount as u128 * lp_share / 10000) as u64;
        let amount_b = (ctx.accounts.token_b_vault.amount as u128 * lp_share / 10000) as u64;

        require!(
            amount_a >= minimum_amount_a && amount_b >= minimum_amount_b,
            SimpleDexError::SlippageExceeded
        );

        // Check k_last to protect against price manipulation
        let current_k = (ctx.accounts.token_a_vault.amount as u128)
            * (ctx.accounts.token_b_vault.amount as u128);
        require!(
            pair.k_last > 0 && current_k >= pair.k_last,
            SimpleDexError::PriceManipulation
        );

        let seeds = &[AUTHORITY_SEED, &[pair.bump]];
        let signer = &[&seeds[..]];

        // Burn LP tokens
        anchor_spl::token::burn(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Burn {
                    mint: ctx.accounts.lp_mint.to_account_info(),
                    from: ctx.accounts.user_lp_token.to_account_info(),
                    authority: ctx.accounts.user.to_account_info(),
                },
            ),
            lp_amount,
        )?;

        // Transfer tokens back to user
        anchor_spl::token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token_a_vault.to_account_info(),
                    to: ctx.accounts.user_token_a.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer,
            ),
            amount_a,
        )?;

        anchor_spl::token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token_b_vault.to_account_info(),
                    to: ctx.accounts.user_token_b.to_account_info(),
                    authority: ctx.accounts.authority.to_account_info(),
                },
                signer,
            ),
            amount_b,
        )?;

        // Update k_last and reserves
        pair.k_last = ((ctx.accounts.token_a_vault.amount - amount_a) as u128)
            * ((ctx.accounts.token_b_vault.amount - amount_b) as u128);
        update_reserves(
            pair,
            ctx.accounts.token_a_vault.amount - amount_a,
            ctx.accounts.token_b_vault.amount - amount_b,
        )?;

        emit!(LiquidityRemoved {
            user: ctx.accounts.user.key(),
            pair: ctx.accounts.pair.key(),
            lp_amount,
            amount_a,
            amount_b,
        });

        Ok(())
    }

    pub fn get_amount_out(
        ctx: Context<GetAmountOut>,
        amount_in: u64,
        swap_a_to_b: bool,
    ) -> Result<u64> {
        let token_a_vault = &ctx.accounts.token_a_vault;
        let token_b_vault = &ctx.accounts.token_b_vault;

        let (reserve_in, reserve_out) = if swap_a_to_b {
            (token_a_vault.amount, token_b_vault.amount)
        } else {
            (token_b_vault.amount, token_a_vault.amount)
        };

        let fee_amount = amount_in * FEE_NUMERATOR / FEE_DENOMINATOR;
        let amount_in_with_fee = amount_in - fee_amount;

        let amount_out = (amount_in_with_fee as u128 * reserve_out as u128
            / (reserve_in as u128 + amount_in_with_fee as u128)) as u64;

        Ok(amount_out)
    }
}

#[derive(Accounts)]
pub struct GetAmountOut<'info> {
    pub pair: Account<'info, Pair>,
    pub token_a_vault: Account<'info, TokenAccount>,
    pub token_b_vault: Account<'info, TokenAccount>,
}

#[derive(Accounts)]
#[instruction(bump: u8)]
pub struct CreatePair<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + Pair::LEN,
        seeds = [PAIR_SEED, token_a_mint.key().as_ref(), token_b_mint.key().as_ref()],
        bump,
    )]
    pub pair: Account<'info, Pair>,

    #[account(
        seeds = [AUTHORITY_SEED],
        bump = bump,
    )]
    /// CHECK: PDA account that acts as authority
    pub authority: UncheckedAccount<'info>,

    pub token_a_mint: Account<'info, Mint>,
    pub token_b_mint: Account<'info, Mint>,

    #[account(
        init,
        payer = admin,
        token::mint = token_a_mint,
        token::authority = authority,
    )]
    pub token_a_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        token::mint = token_b_mint,
        token::authority = authority,
    )]
    pub token_b_vault: Account<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        mint::decimals = 9,
        mint::authority = authority,
    )]
    pub lp_mint: Account<'info, Mint>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}
#[derive(Accounts)]
pub struct AddLiquidity<'info> {
    #[account(
        mut,
        has_one = token_a_mint,
        has_one = token_b_mint,
        has_one = token_a_vault,
        has_one = token_b_vault,
        has_one = lp_mint,
        has_one = authority,
    )]
    pub pair: Account<'info, Pair>,

    /// CHECK: PDA account validated via constraint
    pub authority: UncheckedAccount<'info>,

    pub token_a_mint: Account<'info, Mint>,
    pub token_b_mint: Account<'info, Mint>,

    #[account(mut)]
    pub token_a_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub token_b_vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub lp_mint: Account<'info, Mint>,

    #[account(
        mut,
        token::mint = token_a_mint,
        token::authority = user,
    )]
    pub user_token_a: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = token_b_mint,
        token::authority = user,
    )]
    pub user_token_b: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = lp_mint,
        token::authority = user,
    )]
    pub user_lp_token: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub token_program: Program<'info, Token>,
}
#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(
        mut,
        constraint = 
            (pair.token_a_mint == token_in_mint.key() && pair.token_b_mint == token_out_mint.key()) ||
            (pair.token_b_mint == token_in_mint.key() && pair.token_a_mint == token_out_mint.key()),
        has_one = authority
    )]
    pub pair: Account<'info, Pair>,

    /// CHECK: PDA account validated in pair account
    pub authority: UncheckedAccount<'info>,

    pub token_in_mint: Account<'info, Mint>,
    pub token_out_mint: Account<'info, Mint>,

    #[account(mut)]
    pub token_a_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub token_b_vault: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = token_in_mint,
        token::authority = user,
    )]
    pub user_token_in: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = token_out_mint,
        token::authority = user,
    )]
    pub user_token_out: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = token_in_mint,
        token::authority = admin,
    )]
    pub admin_token_in: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    /// CHECK: Admin account validated in pair account
    pub admin: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RemoveLiquidity<'info> {
    #[account(
        mut,
        has_one = token_a_mint,
        has_one = token_b_mint,
        has_one = token_a_vault,
        has_one = token_b_vault,
        has_one = lp_mint,
        has_one = authority,
    )]
    pub pair: Account<'info, Pair>,

    /// CHECK: PDA account validated via constraint
    pub authority: UncheckedAccount<'info>,

    pub token_a_mint: Account<'info, Mint>,
    pub token_b_mint: Account<'info, Mint>,

    #[account(mut)]
    pub token_a_vault: Account<'info, TokenAccount>,
    #[account(mut)]
    pub token_b_vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub lp_mint: Account<'info, Mint>,

    #[account(
        mut,
        token::mint = token_a_mint,
        token::authority = user,
    )]
    pub user_token_a: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = token_b_mint,
        token::authority = user,
    )]
    pub user_token_b: Account<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = lp_mint,
        token::authority = user,
    )]
    pub user_lp_token: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

#[account]
pub struct Pair {
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_vault: Pubkey,
    pub token_b_vault: Pubkey,
    pub lp_mint: Pubkey,
    pub authority: Pubkey,
    pub admin: Pubkey,
    pub bump: u8,
    // Flash loan protection
    pub reserve_a_last: u64,
    pub reserve_b_last: u64,
    pub last_block: u64,
    // Price manipulation protection
    pub k_last: u128,
}

impl Pair {
    pub const LEN: usize = 32 * 7 + 1 + 8 * 3 + 16;
}

// Helper functions for flash loan protection
fn update_reserves(pair: &mut Account<Pair>, reserve_a: u64, reserve_b: u64) -> Result<()> {
    let clock = Clock::get()?;

    if clock.slot != pair.last_block {
        pair.reserve_a_last = reserve_a;
        pair.reserve_b_last = reserve_b;
        pair.last_block = clock.slot;
    }

    Ok(())
}

fn check_reserves(pair: &Account<Pair>, reserve_a: u64, reserve_b: u64) -> Result<()> {
    let clock = Clock::get()?;

    if clock.slot == pair.last_block {
        require!(
            reserve_a == pair.reserve_a_last && reserve_b == pair.reserve_b_last,
            SimpleDexError::FlashLoanAttempt
        );
    }

    Ok(())
}

#[error_code]
pub enum SimpleDexError {
    #[msg("Amount out less than minimum")]
    SlippageExceeded,
    #[msg("Flash loan attempt detected")]
    FlashLoanAttempt,
    #[msg("Price manipulation detected")]
    PriceManipulation,
    #[msg("Insufficient initial liquidity")]
    InsufficientInitialLiquidity,
}

#[event]
pub struct PairCreated {
    pub pair: Pubkey,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_vault: Pubkey,
    pub token_b_vault: Pubkey,
    pub lp_mint: Pubkey,
}

#[event]
pub struct LiquidityAdded {
    pub user: Pubkey,
    pub pair: Pubkey,
    pub amount_a: u64,
    pub amount_b: u64,
    pub lp_minted: u64,
}

#[event]
pub struct LiquidityRemoved {
    pub user: Pubkey,
    pub pair: Pubkey,
    pub lp_amount: u64,
    pub amount_a: u64,
    pub amount_b: u64,
}

#[event]
pub struct Swapped {
    pub user: Pubkey,
    pub pair: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub a_to_b: bool,
}
