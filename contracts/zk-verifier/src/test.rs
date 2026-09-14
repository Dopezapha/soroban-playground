extern crate std;

use super::*;
use ark_bn254::{Fq, Fq2, G1Affine, G1Projective, G2Affine};
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{BigInteger, PrimeField};
use soroban_sdk::{BytesN, Env};

fn fq_bytes(value: Fq) -> [u8; 32] {
    let encoded = value.into_bigint().to_bytes_be();
    let mut out = [0u8; 32];
    out[32 - encoded.len()..].copy_from_slice(&encoded);
    out
}

fn g1_bytes(env: &Env, point: G1Affine) -> BytesN<64> {
    if point.is_zero() {
        return BytesN::from_array(env, &[0u8; 64]);
    }
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(&fq_bytes(point.x));
    out[32..].copy_from_slice(&fq_bytes(point.y));
    BytesN::from_array(env, &out)
}

fn fq2_bytes(value: Fq2) -> [u8; 64] {
    let mut out = [0u8; 64];
    // Soroban/Ethereum encoding is imaginary component followed by real.
    out[..32].copy_from_slice(&fq_bytes(value.c1));
    out[32..].copy_from_slice(&fq_bytes(value.c0));
    out
}

fn g2_bytes(env: &Env, point: G2Affine) -> BytesN<128> {
    if point.is_zero() {
        return BytesN::from_array(env, &[0u8; 128]);
    }
    let mut out = [0u8; 128];
    out[..64].copy_from_slice(&fq2_bytes(point.x));
    out[64..].copy_from_slice(&fq2_bytes(point.y));
    BytesN::from_array(env, &out)
}

fn fixture(env: &Env) -> (VerificationKey, Proof) {
    let g1 = G1Affine::generator();
    let g2 = G2Affine::generator();
    let a = (G1Projective::from(g1) * ark_bn254::Fr::from(3u64)).into_affine();

    (
        VerificationKey {
            alpha_g1: g1_bytes(env, g1),
            beta_g2: g2_bytes(env, g2),
            gamma_g2: g2_bytes(env, g2),
            delta_g2: g2_bytes(env, g2),
            ic: soroban_sdk::vec![env, g1_bytes(env, g1)],
        },
        Proof {
            a: g1_bytes(env, a),
            b: g2_bytes(env, g2),
            c: g1_bytes(env, g1),
        },
    )
}

fn client(env: &Env) -> ZkVerifierClient<'_> {
    let id = env.register(ZkVerifier, ());
    ZkVerifierClient::new(env, &id)
}

#[test]
fn accepts_valid_pairing_equation() {
    let env = Env::default();
    let client = client(&env);
    let (vk, proof) = fixture(&env);

    assert!(client.verify(&vk, &proof, &Vec::new(&env)));
}

#[test]
fn rejects_invalid_proof_without_trapping() {
    let env = Env::default();
    let client = client(&env);
    let (vk, mut proof) = fixture(&env);
    let g1 = G1Affine::generator();
    let two_g1 = (G1Projective::from(g1) * ark_bn254::Fr::from(2u64)).into_affine();
    proof.a = g1_bytes(&env, two_g1);

    assert!(!client.verify(&vk, &proof, &Vec::new(&env)));
}

#[test]
fn enforces_exact_public_input_count() {
    let env = Env::default();
    let (vk, proof) = fixture(&env);
    let input = BytesN::from_array(&env, &[0u8; 32]);

    assert_eq!(
        ZkVerifier::verify(env.clone(), vk, proof, soroban_sdk::vec![&env, input]),
        Err(Error::InvalidInputCount)
    );
}

#[test]
fn rejects_non_canonical_public_input() {
    let env = Env::default();
    let (mut vk, proof) = fixture(&env);
    vk.ic.push_back(g1_bytes(&env, G1Affine::generator()));
    let modulus = BytesN::from_array(&env, &FR_MODULUS);

    assert_eq!(
        ZkVerifier::verify(env.clone(), vk, proof, soroban_sdk::vec![&env, modulus]),
        Err(Error::NonCanonicalPublicInput)
    );
}

#[test]
fn rejects_invalid_g1_point() {
    let env = Env::default();
    let (vk, mut proof) = fixture(&env);
    proof.a = BytesN::from_array(&env, &[0xff; 64]);

    assert_eq!(
        ZkVerifier::verify(env.clone(), vk, proof, Vec::new(&env)),
        Err(Error::InvalidG1Point)
    );
}

#[test]
fn verifies_a_public_input_msm() {
    let env = Env::default();
    let client = client(&env);
    let g1 = G1Affine::generator();
    let g2 = G2Affine::generator();
    // vk_x = IC0 + 2*IC1 = 3G; alpha = C = G, so A = 5G.
    let five_g1 = (G1Projective::from(g1) * ark_bn254::Fr::from(5u64)).into_affine();
    let vk = VerificationKey {
        alpha_g1: g1_bytes(&env, g1),
        beta_g2: g2_bytes(&env, g2),
        gamma_g2: g2_bytes(&env, g2),
        delta_g2: g2_bytes(&env, g2),
        ic: soroban_sdk::vec![&env, g1_bytes(&env, g1), g1_bytes(&env, g1)],
    };
    let proof = Proof {
        a: g1_bytes(&env, five_g1),
        b: g2_bytes(&env, g2),
        c: g1_bytes(&env, g1),
    };
    let mut two = [0u8; 32];
    two[31] = 2;

    assert!(client.verify(
        &vk,
        &proof,
        &soroban_sdk::vec![&env, BytesN::from_array(&env, &two)]
    ));
}
