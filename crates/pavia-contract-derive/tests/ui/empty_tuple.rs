use pavia_contract::Contract;

#[derive(Contract)]
struct Nothing();

#[derive(Contract)]
enum Signal {
  Stop(),
}

fn main() {}
