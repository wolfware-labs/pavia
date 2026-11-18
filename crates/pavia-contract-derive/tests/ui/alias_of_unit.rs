use pavia_contract::{Contract, TypeTable};

#[derive(Contract)]
struct Nothing(());

#[derive(Contract)]
struct Again(Nothing);

fn main() {
  let mut table = TypeTable::new();
  table.add::<Option<Nothing>>();
  table.add::<Option<Box<Again>>>();
}
