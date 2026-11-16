use pavia_contract::Contract;

#[derive(Contract)]
struct Borrowed<'a> {
  text: &'a str,
}

fn main() {}
