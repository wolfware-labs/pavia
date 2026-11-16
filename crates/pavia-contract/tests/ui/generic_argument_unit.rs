use pavia_contract::{Contract, TypeTable};

#[derive(Contract)]
struct Page<T> {
  items: Vec<T>,
}

fn main() {
  TypeTable::new().add::<Page<()>>();
}
