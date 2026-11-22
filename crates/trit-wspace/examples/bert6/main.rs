//! MiniLM 6 层前向的入口。

mod activation;
mod linear;
mod model;
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
