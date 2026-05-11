mod g502;

pub use g502::G502Driver;

pub fn registered_drivers() -> Vec<Box<dyn super::SupplementalDriver>> {
    vec![Box::new(G502Driver)]
}
