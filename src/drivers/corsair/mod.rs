pub mod gkeys;
mod k95;

pub use k95::CorsairK95Driver;

pub fn registered_drivers() -> Vec<Box<dyn super::SupplementalDriver>> {
    vec![Box::new(CorsairK95Driver)]
}
