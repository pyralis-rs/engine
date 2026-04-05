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

#[cfg(test)]
mod tests {
    use super::estimate_total_gas_cost;

    #[test]
    fn test_estimate_total_gas_cost_matches_op_stack_formula() {
        let tx_data = [0_u8, 1_u8, 2_u8];
        let total = estimate_total_gas_cost(&tx_data, 1_000, 10, 2);

        // l2: 1_000 * 10 = 10_000
        // l1 units: 4 + 16 + 16 = 36, fee: 36 * 2 = 72
        assert_eq!(total, 10_072);
    }

    #[test]
    fn test_estimate_total_gas_cost_saturates_on_overflow() {
        let total = estimate_total_gas_cost(&[1_u8], u64::MAX, u128::MAX, u128::MAX);
        assert_eq!(total, u128::MAX);
    }
}
