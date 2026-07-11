use aether_example_energy_gateway::{EnergyGateway, bundled_load_forecast_contract};
use aether_sdk::domain::FeatureRole;

#[test]
fn bundled_energy_pack_layers_over_the_generic_gateway() {
    let gateway = EnergyGateway::bundled().expect("bundled energy pack must be valid");
    let summary = gateway.pack_summary();

    assert_eq!(summary.id, "energy");
    assert_eq!(summary.name, "Aether Energy");
    assert!(summary.capabilities.iter().any(|model| model == "Battery"));
    assert!(summary.example_channel_count > 0);
    assert_eq!(summary.data_processing_task_count, 2);
    let _ = gateway.application();
}

#[test]
fn bundled_energy_composition_starts_uncommissioned_and_fail_safe() {
    let gateway = EnergyGateway::bundled().expect("bundled energy pack must be valid");
    let summary = gateway.pack_summary();

    assert_eq!(summary.enabled_channel_count, 0);
    assert_eq!(summary.enabled_rule_count, 0);
    assert_eq!(summary.enabled_data_processing_task_count, 0);
    assert!(!summary.example_data_processing_binding_commissioned);
    assert_eq!(summary.enabled_data_processing_binding_count, 0);
    assert!(!summary.auto_load_instances);
}

#[test]
fn bundled_load_contract_is_the_validated_task_exposed_by_the_gateway() {
    let gateway = EnergyGateway::bundled().expect("bundled energy pack must be valid");
    let loaded = bundled_load_forecast_contract().expect("bundled load contract must be valid");

    assert_eq!(gateway.load_forecast_contract(), &loaded);
    assert_eq!(loaded.task().identity().id(), "energy.site-load-forecast");
    assert_eq!(loaded.task().identity().revision(), 1);
    assert_eq!(
        loaded
            .task()
            .features()
            .iter()
            .filter(|feature| feature.role() == FeatureRole::History)
            .count(),
        5
    );
    assert_eq!(
        loaded
            .task()
            .features()
            .iter()
            .filter(|feature| feature.role() == FeatureRole::FutureCovariate)
            .count(),
        4
    );
}
