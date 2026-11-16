#[test]
fn refuses_rust_types_without_a_contract_type() {
  trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
