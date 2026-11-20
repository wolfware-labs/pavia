use pavia_contract::Contract;

#[derive(Contract)]
struct Fixed<const N: usize> {
  values: Vec<u8>,
}

fn main() {}
