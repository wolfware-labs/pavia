use pavia_contract::Contract;

#[derive(Contract)]
struct Member {
  #[pavia(optional)]
  display_name: String,
}

fn main() {}
