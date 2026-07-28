#[test]
#[should_panic(expected = "Error(Contract, #1)")] // VestingError::Underfunded
fn test_create_vesting_underfunded_rejection() {
    let e = Env::default();
    let admin = Address::generate(&e);
    let user = Address::generate(&e);
    let lp_token = e.register_stellar_asset_contract(admin.clone());
    
    let client = PolVestingClient::new(&e, &e.register_contract(None, PolVesting));
    client.initialize(&admin, &lp_token);

    // The contract has 0 balance of lp_token. 
    // Trying to create a vesting for 100 should panic.
    client.create_vesting(&user, &0, &10, &100, &100);
}

#[test]
fn test_create_vesting_success_after_transfer() {
    let e = Env::default();
    // ... standard setup ...
    
    // Admin transfers 100 LP tokens to the contract first
    token::Client::new(&e, &lp_token).mint(&client.address, &100);

    // Now creating vesting for 100 should succeed
    client.create_vesting(&user, &0, &10, &100, &100);
    
    // Assert obligations are 100
    // ...
}
