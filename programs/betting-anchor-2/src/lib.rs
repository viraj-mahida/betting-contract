use anchor_lang::prelude::*;
use anchor_lang::solana_program::{program::invoke, system_instruction};

declare_id!("6JRShtnTuvqR6Ntvir7Dv3FXVRZhA34EMWXW4zJRZfzx");

#[program]
pub mod betting_anchor_2 {
    use super::*;

    pub fn create_market(
        ctx: Context<CreateMarket>,
        question: String,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let creator = &ctx.accounts.creator;
        
        market.creator = creator.key();
        market.question = question;
        market.resolved = false;
        market.outcome = Outcome::Undecided;
        market.total_yes_amount = 0;
        market.total_no_amount = 0;
        
        emit!(MarketCreatedEvent {
            market: market.key(),
            creator: creator.key(),
            question: market.question.clone(),
        });
        
        Ok(())
    }

    pub fn place_bet(
        ctx: Context<PlaceBet>,
        choice: Outcome,
        amount: u64,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let bettor = &ctx.accounts.bettor;

        // Ensure market is not resolved
        require!(!market.resolved, BettingError::MarketAlreadyResolved);
        
        // Ensure amount is greater than 0
        require!(amount > 0, BettingError::InvalidBetAmount);

        // Transfer SOL from bettor to market account
        let transfer_instruction = system_instruction::transfer(
            &bettor.key(),
            &market.key(),
            amount,
        );
        
        invoke(
            &transfer_instruction,
            &[
                bettor.to_account_info(),
                market.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
        )?;

        // Record the bet
        match choice {
            Outcome::Yes => {
                market.total_yes_amount = market.total_yes_amount.checked_add(amount).unwrap();
                
                // Check if bettor already exists in yes_bettors
                let bettor_key = bettor.key();
                let bettor_index = market.yes_bettors.iter().position(|b| b.bettor == bettor_key);
                
                if let Some(index) = bettor_index {
                    // Update existing bettor's amount
                    market.yes_bettors[index].amount = market.yes_bettors[index].amount.checked_add(amount).unwrap();
                } else {
                    // Add new bettor
                    market.yes_bettors.push(Bettor {
                        bettor: bettor_key,
                        amount,
                    });
                }
            },
            Outcome::No => {
                market.total_no_amount = market.total_no_amount.checked_add(amount).unwrap();
                
                // Check if bettor already exists in no_bettors
                let bettor_key = bettor.key();
                let bettor_index = market.no_bettors.iter().position(|b| b.bettor == bettor_key);
                
                if let Some(index) = bettor_index {
                    // Update existing bettor's amount
                    market.no_bettors[index].amount = market.no_bettors[index].amount.checked_add(amount).unwrap();
                } else {
                    // Add new bettor
                    market.no_bettors.push(Bettor {
                        bettor: bettor_key,
                        amount,
                    });
                }
            },
            _ => return Err(BettingError::InvalidBetChoice.into()),
        }

        emit!(BetPlacedEvent {
            market: market.key(),
            bettor: bettor.key(),
            choice,
            amount,
        });

        Ok(())
    }

    pub fn resolve_market(
        ctx: Context<ResolveMarket>,
        outcome: Outcome,
    ) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let creator = &ctx.accounts.creator;

        // Ensure market is not already resolved
        require!(!market.resolved, BettingError::MarketAlreadyResolved);
        
        // Ensure only creator can resolve
        require!(market.creator == creator.key(), BettingError::UnauthorizedAccess);
        
        // Ensure outcome is valid (Yes or No)
        require!(
            outcome == Outcome::Yes || outcome == Outcome::No,
            BettingError::InvalidOutcome
        );

        // Update market state
        market.resolved = true;
        market.outcome = outcome;

        emit!(MarketResolvedEvent {
            market: market.key(),
            outcome,
        });

        Ok(())
    }

    pub fn claim_winnings(ctx: Context<ClaimWinnings>) -> Result<()> {
        let market = &mut ctx.accounts.market;
        let claimant = &ctx.accounts.claimant;
        
        // Ensure market is resolved
        require!(market.resolved, BettingError::MarketNotResolved);
        
        let outcome = market.outcome;
        let claimant_key = claimant.key();
        
        // Find if claimant is a winner and get their amount
        let (is_winner, bettor_amount, bettor_index, winning_total, losing_total) = match outcome {
            Outcome::Yes => {
                if let Some(index) = market.yes_bettors.iter().position(|b| b.bettor == claimant_key) {
                    (true, market.yes_bettors[index].amount, index, market.total_yes_amount, market.total_no_amount)
                } else {
                    (false, 0, 0, 0, 0)
                }
            },
            Outcome::No => {
                if let Some(index) = market.no_bettors.iter().position(|b| b.bettor == claimant_key) {
                    (true, market.no_bettors[index].amount, index, market.total_no_amount, market.total_yes_amount)
                } else {
                    (false, 0, 0, 0, 0)
                }
            },
            _ => return Err(BettingError::InvalidMarketState.into()),
        };
        
        if !is_winner {
            return Err(BettingError::NotAWinner.into());
        }
        
        // Calculate winnings
        let share_of_losers = if winning_total > 0 {
            bettor_amount
                .checked_mul(losing_total)
                .ok_or(BettingError::OverflowError)?
                .checked_div(winning_total)
                .ok_or(BettingError::OverflowError)?
        } else {
            0
        };
        
        let total_winnings = bettor_amount.checked_add(share_of_losers).ok_or(BettingError::OverflowError)?;
        
        // Transfer winnings
        let market_info = market.to_account_info();
        let claimant_info = claimant.to_account_info();
        
        **market_info.try_borrow_mut_lamports()? = market_info.lamports()
            .checked_sub(total_winnings)
            .ok_or(BettingError::InsufficientFunds)?;
            
        **claimant_info.try_borrow_mut_lamports()? = claimant_info.lamports()
            .checked_add(total_winnings)
            .ok_or(BettingError::OverflowError)?;
        
        // Remove claimant from winners list and update totals
        match outcome {
            Outcome::Yes => {
                market.yes_bettors.remove(bettor_index);
                market.total_yes_amount = market.total_yes_amount.checked_sub(bettor_amount).ok_or(BettingError::OverflowError)?;
            },
            Outcome::No => {
                market.no_bettors.remove(bettor_index);
                market.total_no_amount = market.total_no_amount.checked_sub(bettor_amount).ok_or(BettingError::OverflowError)?;
            },
            _ => return Err(BettingError::InvalidMarketState.into()),
        }
        
        emit!(WinningsClaimedEvent {
            market: market.key(),
            claimant: claimant_key,
            amount: total_winnings,
        });
        
        Ok(())
    }
}

#[derive(Accounts)]
#[instruction(question: String)]
pub struct CreateMarket<'info> {
    #[account(
        init,
        payer = creator,
        space = 8 + Market::SPACE,
    )]
    pub market: Account<'info, Market>,
    
    #[account(mut)]
    pub creator: Signer<'info>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct PlaceBet<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,
    
    #[account(mut)]
    pub bettor: Signer<'info>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ResolveMarket<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,
    
    #[account(
        constraint = market.creator == creator.key() @ BettingError::UnauthorizedAccess
    )]
    pub creator: Signer<'info>,
}

#[derive(Accounts)]
pub struct ClaimWinnings<'info> {
    #[account(mut)]
    pub market: Account<'info, Market>,
    
    #[account(mut)]
    pub claimant: Signer<'info>,
    
    pub system_program: Program<'info, System>,
}

#[account]
pub struct Market {
    pub creator: Pubkey,          // Creator of the market
    pub question: String,         // Question being bet on
    pub resolved: bool,           // Whether the market has been resolved
    pub outcome: Outcome,         // Outcome of the market
    pub total_yes_amount: u64,    // Total amount bet on "Yes" 
    pub total_no_amount: u64,     // Total amount bet on "No"
    pub yes_bettors: Vec<Bettor>, // List of "Yes" bettors
    pub no_bettors: Vec<Bettor>,  // List of "No" bettors
}

impl Market {
    pub const SPACE: usize = 8 +      // id: u64
                            32 +      // creator: Pubkey
                            4 + 256 + // question: String (max 256 chars)
                            1 +       // resolved: bool
                            1 +       // outcome: Outcome
                            8 +       // total_yes_amount: u64
                            8 +       // total_no_amount: u64
                            4 + (32 + 8) * 20 + // yes_bettors: Vec<Bettor> (max 20 bettors)
                            4 + (32 + 8) * 20;  // no_bettors: Vec<Bettor> (max 20 bettors)
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Undecided,
    Yes,
    No,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct Bettor {
    pub bettor: Pubkey,
    pub amount: u64,
}

#[event]
pub struct MarketCreatedEvent {
    pub market: Pubkey,
    pub creator: Pubkey,
    pub question: String,
}

#[event]
pub struct BetPlacedEvent {
    pub market: Pubkey,
    pub bettor: Pubkey,
    pub choice: Outcome,
    pub amount: u64,
}

#[event]
pub struct MarketResolvedEvent {
    pub market: Pubkey,
    pub outcome: Outcome,
}

#[event]
pub struct WinningsClaimedEvent {
    pub market: Pubkey,
    pub claimant: Pubkey,
    pub amount: u64,
}

#[error_code]
pub enum BettingError {
    #[msg("Market is already resolved")]
    MarketAlreadyResolved,
    
    #[msg("Market is not yet resolved")]
    MarketNotResolved,
    
    #[msg("Invalid bet amount")]
    InvalidBetAmount,
    
    #[msg("Invalid bet choice")]
    InvalidBetChoice,
    
    #[msg("Invalid outcome")]
    InvalidOutcome,
    
    #[msg("Unauthorized access")]
    UnauthorizedAccess,
    
    #[msg("Not a winner in this market")]
    NotAWinner,
    
    #[msg("Invalid market state")]
    InvalidMarketState,
    
    #[msg("Insufficient funds")]
    InsufficientFunds,
    
    #[msg("Arithmetic overflow")]
    OverflowError,
} 