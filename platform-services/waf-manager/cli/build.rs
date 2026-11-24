fn main() {
    let schema = "../../graph/supergraph/schema.graphql";
    println!("cargo:rerun-if-changed={schema}");
    cynic_codegen::register_schema("graph")
        .from_sdl_file(schema)
        .expect("the supergraph schema is readable")
        .as_default()
        .expect("the supergraph schema registers");
}
