use pavia_contract::Contract;

#[derive(Contract)]
#[pavia(crate = "pavia_contract::<u8>")]
struct Message {
  text: String,
}

fn main() {}
