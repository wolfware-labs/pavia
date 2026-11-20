use pavia_contract::Contract;

#[derive(Contract)]
union Bits {
  int: u32,
  float: f32,
}

fn main() {}
