use imp_llm::model::ModelRegistry;

pub(crate) fn run_list() {
    let registry = ModelRegistry::with_builtins();
    let models = registry.list();

    println!(
        "{:<40} {:<12} {:>8} {:>10} {:>10}",
        "MODEL", "PROVIDER", "CONTEXT", "$/M IN", "$/M OUT"
    );
    println!("{}", "-".repeat(84));

    for m in models {
        println!(
            "{:<40} {:<12} {:>7}k {:>10} {:>10}",
            m.id,
            m.provider,
            m.context_window / 1000,
            format_price(m.pricing.input_per_mtok),
            format_price(m.pricing.output_per_mtok),
        );
    }
}

fn format_price(price: f64) -> String {
    if price == 0.0 {
        "n/a".to_string()
    } else {
        format!("${price:.2}")
    }
}
