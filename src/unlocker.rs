use std::sync::*;
use daemon_client::*;
use config::*;
use db::*;
use app::App;
use db::models::*;

pub struct Unlocker {
  app: Arc<App>,
}

impl Unlocker {
  pub fn new(app: Arc<App>) -> Unlocker {
    Unlocker {
      app,
    }
  }

  pub fn refresh(&self) {
    self.process_blocks();
    self.process_payments();
  }

  pub fn process_blocks(&self) {
    let blocks = self.app.db.pending_submitted_blocks();
    for FoundBlock { block_id, .. } in blocks {
      let header_for_hash = self.app.daemon.get_block_header(&block_id);
      match header_for_hash {
        Ok(header) => {
          if header.hash != block_id || header.orphan_status {
            self.app.db.block_status(&block_id, BlockStatus::Orphaned);
          }
          else if header.depth >= 60 {
            self.assign_balances(&block_id, header.reward);
          }
          else {
            self.app.db.block_progress(&block_id, header.depth);
          }
        },
        Err(err) => {
          warn!("Unexpected result from daemon: {:?}", err);
        }
      }
    }
  }

  /// Appends donation fee shares, and returns the new total count of shares.  The pool fee is
  /// included in the returned total share count, but not appended to the share counts array, since
  /// there is no transaction needed to move funds from the pool to itself.
  fn append_fees(share_counts: &mut Vec<BlockShare>, config: &Config) -> u64 {
    let miner_shares: u64 = share_counts.iter().map(|share| share.shares).sum();
    let dev_fee_percent: f64 = config.donations.iter().map(|donation| donation.percentage).sum();
    let total_fee_ratio: f64 = (config.pool_fee + dev_fee_percent) / 100.0;
    let miner_share_portion: f64 = 1.0 - total_fee_ratio;
    let total_shares = (miner_shares as f64 * (1.0 / miner_share_portion)).round() as u64;
    for &Donation { ref address, ref percentage } in &config.donations {
      share_counts.push(BlockShare {
        shares: (total_shares as f64 * (percentage / 100.0)).round() as u64,
        address: address.to_owned(),
        is_fee: true
      });
    }
    total_shares
  }

  pub fn assign_balances(&self, block_id: &str, reward: u64) {
    let network_fee = self.app.config.network_transaction_fee;
    let adjusted_reward = if reward > 10 * network_fee {
      reward - network_fee
    } else {
      error!(
        "The value for network_transaction_fee in the config is unusually high, so cryptosmelt will \
         attempt to distribute balances without accounting for the network fee.  Please double \
         check that value in the config - otherwise there is a chance of the pool getting stuck \
         waiting to have enough funds to pay miners and cover the transaction fee.  Feel free to \
         open an issue on cryptosmelt's github if you run into troubles here."
      );
      reward
    };
    warn!(
      "Assigning balances for found block.  Reward: {}, Reward after network fee: {}.",
      reward, adjusted_reward,
    );
    let shares = self.app.db.unpaid_shares();
    let mut share_counts: Vec<BlockShare> = shares.iter().map(|share| {
      BlockShare {
        shares: share.shares as u64,
        address: share.address.to_owned(),
        is_fee: false,
      }
    }).collect();
    let total_shares = Self::append_fees(&mut share_counts, &self.app.config);
    self.app.db.distribute_balances(adjusted_reward, block_id, share_counts, total_shares);
  }

  pub fn process_payments(&self) {
    let payment_units_per_currency: f64 = 1e12;
    let min_payment = (self.app.config.min_payment * payment_units_per_currency) as i64;

    let mut transfers = vec![];
    let balance_totals = self.app.db.miner_balance_totals();
    let pending_payments = balance_totals.iter()
      .filter(|payment| payment.amount > min_payment);
    for &MinerBalanceTotal { ref amount, ref address } in pending_payments {
      if self.app.address_pattern.is_match(&address) {
        let micro_denomination = self.app.config.payment_denomination * payment_units_per_currency;
        let mut payment = *amount as u64;
        payment -= payment % (micro_denomination as u64);
        info!("Payment {}, denomination {}", payment, micro_denomination);
        if payment > 0 {
          transfers.push(Transfer {
            address: address.to_owned(),
            amount: payment,
          });
        }
      }
      else {
        info!("Skipping payment of {} to {} due to malformed address.", amount, address);
      }
    }
    if transfers.len() == 0 {
      return;
    }
    info!("Transfers: {:?}", &transfers);
    if self.app.db.is_connected() {
      // It's important to check that we have a connection before transferring, since not having
      // a DB connection after a transfer is a dangerous case.  There is still the chance that we
      // could lose connection during the transfer, but this is as close as we can get to an atomic
      // transaction between our database and the daemon.
      match self.app.daemon.transfer(&transfers) {
        Ok(result) => {
          // Some flavors of the simplewallet RPC API return a fee, because the fee gets
          // automatically determined by simplewallet.  On others, simplewallet simply uses the fee
          // value it receives.  We assume that if simplewallet does not give us back a value for
          // the transaction fee, then it has used the value we fed it.
          let transaction_fee = result.fee.unwrap_or(self.app.config.network_transaction_fee);
          self.app.db.log_transfers(&transfers, &result.tx_hash, transaction_fee);
        },
        Err(err) => error!("Failed to initiate transfer: {:?}", err),
      }
    }
    else {
      warn!("Miners have payable balances, but the connection was lost while computing them.")
    }
  }
}

#[cfg(test)]
mod tests {
  use unlocker::*;

  #[test]
  fn test_fee_percentages() {
    let fee_config = Config {
      hash_type: String::new(),
      log_level: String::new(),
      log_file: String::new(),
      daemon_url: String::new(),
      wallet_url: String::new(),
      payment_mixin: 0,
      network_transaction_fee: 0,
      min_payment: 0.0,
      payment_denomination: 0.0,
      pool_wallet: "pool".to_owned(),
      pool_fee: 10.0,
      donations: vec![Donation {
        address: "dev".to_owned(),
        percentage: 15.0,
      }],
      ports: Vec::new(),
    };
    let mut example_shares = vec![BlockShare {
      shares: 150000,
      address: "miner1".to_owned(),
      is_fee: false,
    }, BlockShare {
      shares: 50000,
      address: "miner2".to_owned(),
      is_fee: false,
    }];
    let total_shares = Unlocker::append_fees(&mut example_shares, &fee_config);
    // Because the total fee percentage is 25% (an unrealistic but easy-to-reason-about number), 75%
    // of shares should go to the miners.
    assert_eq!(total_shares * 3 / 4, 150000 + 50000);
    // 90% of the shares should be allocated for transactions, the 10% pool fee in our scenario just
    // sits in the pool wallet.
    let distributed_shares: u64 = example_shares.iter().map(|share| share.shares).sum();
    assert_eq!(total_shares * 9 / 10, distributed_shares);
  }
}