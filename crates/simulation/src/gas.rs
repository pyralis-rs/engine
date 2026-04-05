//! Base L2 gas-cost helpers.

/// Estimates total cost on OP Stack chains.
///
/// Formula:
/// `total_cost = l2_execution_fee + l1_data_fee`
/// where:
/// - `l2_execution_fee = gas_used * l2_base_fee`
/// - `l1_data_fee = calldata_gas_units * l1_blob_base_fee`
pub fn estimate_total_gas_cost(
    tx_data: &[u8],
    gas_used: u64,
    l2_base_fee: u128,
    l1_blob_base_fee: u128,
) -> u128 {
    let l2_execution_fee = u128::from(gas_used).saturating_mul(l2_base_fee);
    let l1_data_gas_units = estimate_l1_data_gas_units(tx_data);
    let l1_data_fee = l1_data_gas_units.saturating_mul(l1_blob_base_fee);

    l2_execution_fee.saturating_add(l1_data_fee)
}

fn estimate_l1_data_gas_units(tx_data: &[u8]) -> u128 {
    tx_data
        .iter()
        .map(|byte| if *byte == 0 { 4_u128 } else { 16_u128 })
        .sum()
}
