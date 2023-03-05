use ethers::prelude::{rand::Rng, *};
use eyre::Result;
use std::{collections::HashMap, sync::Arc};

use crate::{
    contracts::stake_registry::{StakeRegistry, StakeRegistryEvents},
    topology::Topology,
};

pub type Overlay = [u8; 32];

const STAKEREGISTRY_START_BLOCK: u64 = 25527075;

struct Player {
    stake: U256,
}

pub struct Game {
    players: HashMap<Overlay, Player>,
    round_length: u64,
    depth: u32,
}

impl Game {
    pub async fn new(
        registry_address: H160,
        client: Arc<Provider<Http>>,
        store: &Topology,
    ) -> Result<Self> {
        // StakeRegistry contract
        let contract = StakeRegistry::new(registry_address, client.clone());

        let mut players: HashMap<Overlay, Player> = HashMap::new();

        // Subscribe to the StakeUpdated event
        let events = contract.events().from_block(STAKEREGISTRY_START_BLOCK);
        let logs = events.query().await?;

        // iterate over the events
        for log in logs.iter() {
            if let StakeRegistryEvents::StakeUpdatedFilter(f) = log {
                // get the overlay address
                let overlay = f.overlay;
                // get the stake
                let stake = f.stake_amount;

                // add the overlay address and stake to the hashmap if the stake is greater than 0
                // if the overlay address already exists, add the new stake to the existing stake
                if !stake.is_zero() {
                    players
                        .entry(overlay)
                        .and_modify(|e| e.stake += stake)
                        .or_insert(Player { stake });
                }
            }
        }

        Ok(Self {
            players,
            round_length: 152,
            depth: store.depth,
        })
    }

    /// Returns a vector of players in the game sorted by overlay address and optionally filtered by neighbourhood.
    pub fn view_by_radius(
        &self,
        radius: Option<u32>,
        target: Option<u32>,
    ) -> Vec<(Overlay, U256, u32)> {
        let mut players: Vec<(Overlay, U256, u32)> = Vec::new();
        let store = Topology::new(radius.unwrap_or(8));

        for (overlay, player) in self.players.iter() {
            if player.stake > U256::from(0) {
                players.push((*overlay, player.stake, store.get_neighbourhood(*overlay)));
            }
        }

        // sort the vector by overlay address
        players.sort_by(|a, b| a.0.cmp(&b.0));

        // if a target neighbourhood is specified, filter the players by neighbourhood
        if let Some(target) = target {
            players.retain(|(_, _, r)| *r == target);
        }

        players
    }

    /// Returns a vector of neighbourhoods and their population in the game.
    /// There may optionally be a filter to only return neighbourhoods within a range.
    /// The vector is sorted ascending by population.
    pub fn view_by_neighbourhood_population(
        &self,
        radius: Option<u32>,
        filter: Option<(u32, u32)>,
    ) -> Vec<(u32, u32)> {
        let t = Topology::new(radius.unwrap_or(8));

        // Create a hashmap to hold the neighbourhoods and their population
        let mut neighbourhoods: HashMap<u32, u32> = HashMap::new();

        // Get the view of the game by radius
        let view = self.view_by_radius(radius, None);

        // Iterate over the view and count the number of players in each neighbourhood
        for (_, _, neighbourhood) in view {
            match filter {
                Some((lower, upper)) => {
                    if neighbourhood < lower || neighbourhood >= upper {
                        continue;
                    }
                }
                None => (),
            }
            neighbourhoods
                .entry(neighbourhood)
                .and_modify(|e| *e += 1)
                .or_insert(1);
        }

        // Convert the hashmap to a vector
        let mut neighbourhoods: Vec<(u32, u32)> = neighbourhoods.into_iter().collect();

        // Fill in any missing neighbourhoods with a population of 0
        // This is necessary because the neighbourhoods are not necessarily contiguous
        // And apply the filter if one is specified
        for neighbourhood in 0..t.num_neighbourhoods() {
            match filter {
                Some((lower, upper)) => {
                    if neighbourhood < lower || neighbourhood >= upper {
                        continue;
                    }
                }
                None => (),
            }
            if !neighbourhoods.iter().any(|(n, _)| *n == neighbourhood) {
                neighbourhoods.push((neighbourhood, 0));
            }
        }

        // Sort the vector by population
        neighbourhoods.sort_by(|a, b| a.1.cmp(&b.1));

        neighbourhoods
    }

    /// Return the set of neighbourhoods with the lowest population.
    pub fn lowest_population_neighbourhoods(
        &self,
        radius: Option<u32>,
        filter: Option<(u32, u32)>,
    ) -> (u32, Vec<u32>) {
        let neighbourhoods = self.view_by_neighbourhood_population(radius, filter);

        let mut lowest_neighbourhoods: Vec<u32> = Vec::new();

        // Iterate over the neighbourhoods and add the neighbourhoods with the lowest population to the vector
        for (neighbourhood, population) in &neighbourhoods {
            if population == &neighbourhoods[0].1 {
                lowest_neighbourhoods.push(*neighbourhood);
            }
        }

        (neighbourhoods[0].1, lowest_neighbourhoods)
    }

    /// A recursive function that finds the optimum neighbourhood to place a new player.
    /// The optimum neighbourhood is the neighbourhood with the lowest population.
    ///
    /// 1. Get the lowest population neighbourhoods.
    /// 2. If there is a tie, choose a random neighbourhood from the set of lowest population neighbourhoods.
    /// 3. Recursively call the function with increasing radius until a neighbourhood is found with a population of 0.
    pub fn find_optimum_neighbourhood_recurse(
        &self,
        radius: Option<u32>,
        filter: Option<(u32, u32)>,
    ) -> (u32, u32) {
        let (population, neighbourhoods) = self.lowest_population_neighbourhoods(radius, filter);

        // If there is a tie, choose a random neighbourhood from the set of lowest population neighbourhoods
        let neighbourhood = match neighbourhoods.len() > 1 {
            false => neighbourhoods[0],
            true => {
                let mut rng = rand::thread_rng();
                neighbourhoods[rng.gen_range(0..neighbourhoods.len())]
            }
        };

        // If the population is 0, return the neighbourhood
        if population == 0 {
            (radius.unwrap(), neighbourhood)
        } else {
            // If the population is not 0, recursively call the function with an increased radius
            self.find_optimum_neighbourhood_recurse(
                Some(radius.unwrap() + 1),
                Some((2 * neighbourhood, (2 * (neighbourhood + 1)))),
            )
        }
    }

    /// Find the optimum neighbourhood to place a new player.
    /// The optimum neighbourhood is the neighbourhood with the lowest population.
    pub fn find_optimum_neighbourhood(&self) -> (u32, u32) {
        self.find_optimum_neighbourhood_recurse(Some(self.depth), None)
    }

    /// Print the game stats
    pub fn stats(&self) {
        let view = self.view_by_radius(Some(self.depth), None);

        let store = Topology::new(self.depth);
        let num_neighbourhoods = store.num_neighbourhoods();

        // Do statistical analysis per neighbourhood. Calculate:
        // - total number of players
        // - total stake
        // - average stake

        println!("Neighbourhood stats:");
        for neighbourhood in 0..num_neighbourhoods {
            let mut total_stake = U256::from(0);
            let mut total_players = 0;

            for (_, stake, r) in view.iter() {
                if *r == neighbourhood {
                    total_stake += *stake;
                    total_players += 1;
                }
            }

            // guard against division by zero
            match total_players == 0 {
                true => println!(
                    "Neighbourhood {}/{}: 0 players",
                    neighbourhood,
                    num_neighbourhoods - 1
                ),
                false => {
                    println!(
                        "Neighbourhood {}/{}: {} players, total stake: {}, avg stake: {}",
                        neighbourhood,
                        num_neighbourhoods - 1,
                        total_players,
                        total_stake,
                        total_stake / U256::from(total_players)
                    );
                }
            }
        }

        println!("{}", self);

        let mut total_stake = U256::from(0);
        let mut total_players = 0;
        let mut neighbourhoods: HashMap<u32, u32> = HashMap::new();

        for (_, stake, neighbourhood) in view {
            total_stake += stake;
            total_players += 1;

            *neighbourhoods.entry(neighbourhood).or_insert(0) += 1;
        }

        println!("Total players: {}", total_players);
        println!("Total stake: {}", total_stake);
        println!("Average stake: {}", total_stake / U256::from(total_players));
        println!(
            "Average neighbourhood population: {}",
            total_players / num_neighbourhoods
        );

        println!(
            "Lowest neighbourhoods: {:?}",
            self.lowest_population_neighbourhoods(None, None)
        );

        println!(
            "Optimum neighbourhood: {:?}",
            self.find_optimum_neighbourhood()
        );
    }
}

impl std::fmt::Display for Game {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let view = self.view_by_radius(Some(self.depth), None);

        writeln!(f, "overlay,stake,neighbourhood")?;
        for (overlay, stake, neighbourhood) in view {
            writeln!(f, "{},{:?},{}", hex::encode(overlay), stake, neighbourhood)?;
        }

        Ok(())
    }
}
