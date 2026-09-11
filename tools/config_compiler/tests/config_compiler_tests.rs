use bexos_config_compiler::compile;

#[test]
fn output_is_deterministic() {
    let source = "def product():\n  return {\"z\": 1, \"a\": \"text\"}\n";
    let first = compile("product.star", source, "product", &[]).unwrap();
    let second = compile("product.star", source, "product", &[]).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, "a: \"text\"\nz: 1\n");
}
