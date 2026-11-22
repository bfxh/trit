use super::route::route;

pub fn run() {
    route(&std::env::args().skip(1).collect::<Vec<String>>())
}
