use std::collections::HashMap;

use pavia_contract::Contract;

#[derive(Contract)]
struct Scores {
  by_player: HashMap<u32, String>,
}

fn main() {}
