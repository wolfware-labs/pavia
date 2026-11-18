use pavia_contract::Contract;

#[derive(Contract)]
#[pavia(open)]
struct Closed {
  value: u32,
}

#[derive(Contract)]
enum Choice {
  #[pavia(optional)]
  One,
}

#[derive(Contract)]
struct Wrapped(#[pavia(rename = "inner")] u32);

#[derive(Contract)]
struct Located {
  #[pavia(crate = "pavia_contract")]
  value: u32,
}

#[derive(Contract)]
#[pavia(sorted)]
struct Unknown {
  value: u32,
}

fn main() {}
