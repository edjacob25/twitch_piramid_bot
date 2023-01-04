use rand::distributions::{Distribution, Standard};
use rand::Rng;

pub enum PyramidAction {
    DoNothing,
    Steal,
    Destroy,
}

impl Distribution<PyramidAction> for Standard {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> PyramidAction {
        match rng.gen_range(0..=9) {
            0..=3 => PyramidAction::DoNothing,
            4..=7 => PyramidAction::Steal,
            _ => PyramidAction::Destroy,
        }
    }
}
