use serde::{Deserialize, Serialize};

use crate::model::ModelPricing;

/// Token usage from a single LLM request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    /// Tokens consumed by the input prompt.
    pub input_tokens: u32,
    /// Tokens generated in the output.
    pub output_tokens: u32,
    /// Tokens served from the prompt cache.
    pub cache_read_tokens: u32,
    /// Tokens written into the prompt cache.
    pub cache_write_tokens: u32,
}

/// Dollar cost breakdown for a request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Cost {
    /// Cost of input tokens.
    pub input: f64,
    /// Cost of output tokens.
    pub output: f64,
    /// Cost of cache-read tokens.
    pub cache_read: f64,
    /// Cost of cache-write tokens.
    pub cache_write: f64,
    /// Sum of all cost components.
    pub total: f64,
}

impl Usage {
    /// Raw total tokens across input and output.
    pub fn raw_total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }

    /// Cache-adjusted total tokens for comparing effective prompt cost/volume.
    pub fn effective_total_tokens(&self) -> u32 {
        self.input_tokens
            .saturating_sub(self.cache_read_tokens)
            .saturating_add(self.output_tokens)
    }

    /// Total tokens across input and output (excludes cache).
    pub fn total_tokens(&self) -> u32 {
        self.raw_total_tokens()
    }

    /// Calculate dollar cost given a model's pricing.
    pub fn cost(&self, pricing: &ModelPricing) -> Cost {
        let input = self.input_tokens as f64 * pricing.input_per_mtok / 1_000_000.0;
        let output = self.output_tokens as f64 * pricing.output_per_mtok / 1_000_000.0;
        let cache_read = self.cache_read_tokens as f64 * pricing.cache_read_per_mtok / 1_000_000.0;
        let cache_write =
            self.cache_write_tokens as f64 * pricing.cache_write_per_mtok / 1_000_000.0;
        let total = input + output + cache_read + cache_write;
        Cost {
            input,
            output,
            cache_read,
            cache_write,
            total,
        }
    }

    /// Accumulate another usage into this one.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

impl Cost {
    /// Accumulate another cost breakdown into this one.
    pub fn add(&mut self, other: &Cost) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.total += other.total;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_tokens_sums_input_and_output() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 200,
            cache_write_tokens: 10,
        };
        assert_eq!(usage.total_tokens(), 150);
    }

    #[test]
    fn raw_and_effective_totals_distinguish_cached_input() {
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 80,
            cache_write_tokens: 10,
        };
        assert_eq!(usage.raw_total_tokens(), 150);
        assert_eq!(usage.total_tokens(), 150);
        assert_eq!(usage.effective_total_tokens(), 70);
    }

    #[test]
    fn effective_total_saturates_when_cache_exceeds_input() {
        let usage = Usage {
            input_tokens: 20,
            output_tokens: 5,
            cache_read_tokens: 30,
            cache_write_tokens: 0,
        };
        assert_eq!(usage.effective_total_tokens(), 5);
    }

    #[test]
    fn cost_calculation_matches_expected() {
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_read_tokens: 200_000,
            cache_write_tokens: 100_000,
        };
        let pricing = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.3,
            cache_write_per_mtok: 3.75,
        };
        let cost = usage.cost(&pricing);

        // 1M input * $3/Mtok = $3.00
        assert!((cost.input - 3.0).abs() < f64::EPSILON);
        // 500k output * $15/Mtok = $7.50
        assert!((cost.output - 7.5).abs() < f64::EPSILON);
        // 200k cache_read * $0.30/Mtok = $0.06
        assert!((cost.cache_read - 0.06).abs() < f64::EPSILON);
        // 100k cache_write * $3.75/Mtok = $0.375
        assert!((cost.cache_write - 0.375).abs() < f64::EPSILON);
        // total = 3.0 + 7.5 + 0.06 + 0.375 = 10.935
        assert!((cost.total - 10.935).abs() < 1e-10);
    }

    #[test]
    fn cost_zero_for_zero_usage() {
        let usage = Usage::default();
        let pricing = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            cache_read_per_mtok: 0.3,
            cache_write_per_mtok: 3.75,
        };
        let cost = usage.cost(&pricing);
        assert!((cost.total).abs() < f64::EPSILON);
    }

    #[test]
    fn add_accumulates_all_fields() {
        let mut a = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 5,
        };
        let b = Usage {
            input_tokens: 200,
            output_tokens: 100,
            cache_read_tokens: 20,
            cache_write_tokens: 10,
        };
        a.add(&b);
        assert_eq!(
            a,
            Usage {
                input_tokens: 300,
                output_tokens: 150,
                cache_read_tokens: 30,
                cache_write_tokens: 15,
            }
        );
    }

    #[test]
    fn cost_add_accumulates_all_fields() {
        let mut a = Cost {
            input: 1.0,
            output: 2.0,
            cache_read: 0.5,
            cache_write: 0.25,
            total: 3.75,
        };
        let b = Cost {
            input: 0.5,
            output: 1.5,
            cache_read: 0.25,
            cache_write: 0.75,
            total: 3.0,
        };
        a.add(&b);
        assert_eq!(
            a,
            Cost {
                input: 1.5,
                output: 3.5,
                cache_read: 0.75,
                cache_write: 1.0,
                total: 6.75,
            }
        );
    }
}
