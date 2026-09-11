use bexos_trusty_client::{keymint_shared_secret as sharing, protocol::TrustyWireError};
use kmr_wire::{
    AsCborValue, ComputeSharedSecretResponse, GetSharedSecretParametersResponse, PerformOpReq,
    PerformOpResponse, PerformOpRsp, sharedsecret::SharedSecretParameters,
};

fn response(value: PerformOpRsp) -> Vec<u8> {
    PerformOpResponse {
        error_code: 0,
        rsp: Some(value),
    }
    .into_vec()
    .unwrap()
}

#[test]
fn single_trusty_participant_is_preserved_in_negotiation() {
    assert!(matches!(
        PerformOpReq::from_slice(&sharing::get_parameters().unwrap()).unwrap(),
        PerformOpReq::SharedSecretGetSharedSecretParameters(_)
    ));
    let parameters = SharedSecretParameters {
        seed: vec![],
        nonce: vec![0x45; 32],
    };
    let bytes = response(PerformOpRsp::SharedSecretGetSharedSecretParameters(
        GetSharedSecretParametersResponse {
            ret: parameters.clone(),
        },
    ));
    let request = sharing::compute_for_single_instance(&bytes).unwrap();
    let PerformOpReq::SharedSecretComputeSharedSecret(request) =
        PerformOpReq::from_slice(&request).unwrap()
    else {
        panic!("wrong negotiation command");
    };
    assert_eq!(request.params, vec![parameters]);
    let check = response(PerformOpRsp::SharedSecretComputeSharedSecret(
        ComputeSharedSecretResponse {
            ret: vec![0x67; 32],
        },
    ));
    assert_eq!(sharing::decode_check(&check).unwrap(), [0x67; 32]);
}

#[test]
fn malformed_parameters_and_secure_failures_are_not_accepted() {
    for (seed, nonce) in [(vec![0; 31], vec![0; 32]), (vec![], vec![0; 31])] {
        let bytes = response(PerformOpRsp::SharedSecretGetSharedSecretParameters(
            GetSharedSecretParametersResponse {
                ret: SharedSecretParameters { seed, nonce },
            },
        ));
        assert_eq!(
            sharing::compute_for_single_instance(&bytes),
            Err(TrustyWireError::InvalidResponse)
        );
    }
    let short = response(PerformOpRsp::SharedSecretComputeSharedSecret(
        ComputeSharedSecretResponse { ret: vec![0; 31] },
    ));
    assert_eq!(
        sharing::decode_check(&short),
        Err(TrustyWireError::InvalidResponse)
    );
    assert_eq!(
        sharing::compute_for_single_instance(&short),
        Err(TrustyWireError::InvalidResponse)
    );
    let error = PerformOpResponse {
        error_code: -49,
        rsp: None,
    }
    .into_vec()
    .unwrap();
    assert_eq!(
        sharing::decode_check(&error),
        Err(TrustyWireError::SecureService(-49))
    );
}
