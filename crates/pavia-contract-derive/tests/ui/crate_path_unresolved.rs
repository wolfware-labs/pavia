use pavia_contract::Contract;

#[derive(Contract)]
#[pavia(crate = "nowhere::contract")]
struct Message {
  text: String,
}

fn main() {}
