use pavia_contract::Contract;

#[derive(Contract)]
struct Ping {
  reply: Option<()>,
}

fn main() {}
