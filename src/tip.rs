use std::cmp::max;

pub struct TipCalculationParams {
    pub tip_boost: f64,
    pub minimum_tip: u64,
}

impl Default for TipCalculationParams {
    fn default() -> Self {
        Self {
            tip_boost: 1.0,
            minimum_tip: 0,
        }
    }
}

impl TipCalculationParams {
    pub fn calculate_tip(&self, current_median_tip: u64) -> u64 {
        let scaled_median_tip = (current_median_tip as f64 * self.tip_boost) as u64;
        max(scaled_median_tip, self.minimum_tip)
    }
}

#[cfg(test)]
mod tests {
    use crate::tip::TipCalculationParams;

    #[test]
    fn boost() {
        let params = TipCalculationParams {
            tip_boost: 2.0,
            minimum_tip: 0,
        };

        assert_eq!(params.calculate_tip(1), 2);
    }

    #[test]
    fn minimum() {
        let params = TipCalculationParams {
            tip_boost: 1.0,
            minimum_tip: 2,
        };

        assert_eq!(params.calculate_tip(1), 2);
    }

    #[test]
    fn complex() {
        let params = TipCalculationParams {
            tip_boost: 3.0,
            minimum_tip: 3,
        };

        assert_eq!(params.calculate_tip(2), 6);
    }
}
