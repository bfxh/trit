//! MiniLM 6 层前向的入口。
mod activation;
mod gelu;
mod linear;
mod model;
mod quantize_input;
mod report;
mod run;
mod scales;
mod sidecar;
mod sidecar_layer;
mod stats;
mod sts;
fn main() {
    run::run();
}
