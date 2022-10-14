use rand::distributions::{Distribution, Standard};
use rand::Rng;

pub enum PyramidAction {
    DoNothing,
    Steal,
    Destroy,
}

impl Distribution<PyramidAction> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> PyramidAction {
        match rng.gen_range(0..=2) {
            0 => PyramidAction::DoNothing,
            1 => PyramidAction::Steal,
            _ => PyramidAction::Destroy,
        }
    }
}
