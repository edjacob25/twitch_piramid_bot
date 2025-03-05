use rand::distr::{Distribution, StandardUniform};
use rand::Rng;

pub enum PyramidAction {
    DoNothing,
    Steal,
    Destroy,
}

impl Distribution<PyramidAction> for StandardUniform {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> PyramidAction {
        match rng.random_range(0..=9) {
            0..=3 => PyramidAction::DoNothing,
            4..=7 => PyramidAction::Steal,
            _ => PyramidAction::Destroy,
        }
    }
}
