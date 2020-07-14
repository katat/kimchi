/*********************************************************************************************************

This source file benchmarks the constraints for the Poseidon hash permutations

**********************************************************************************************************/

use commitment_dlog::{srs::SRS, commitment::CommitmentCurve};
use oracle::{poseidon::*, sponge::{DefaultFqSponge, DefaultFrSponge}};
use plonk_circuits::{wires::GateWires, gate::CircuitGate, constraints::ConstraintSystem};
use algebra::{tweedle::{dum::{Affine, TweedledumParameters}, fq::Fq}, One, Zero, UniformRand};
use plonk_protocol_dlog::{prover::{ProverProof}, index::{Index, SRSSpec}};
use std::{io, io::Write};
use groupmap::GroupMap;
use std::time::Instant;
use colored::Colorize;
use rand_core::OsRng;

const PERIOD: usize = ROUNDS_FULL + 1;
const MAX_SIZE: usize = 40000; // max size of poly chunks
const NUM_POS: usize = 256; // number of Poseidon hashes in the circuit
const N: usize = PERIOD * NUM_POS; // Plonk domain size

#[test]
fn poseidon_tweedledum()
{
    let c = &oracle::tweedle::fq::params().round_constants;

    let z = Fq::zero();
    let p = Fq::one();

    // circuit gates

    let mut i = 0;
    let mut gates: Vec<CircuitGate::<Fq>> = Vec::with_capacity(N);

    // custom constraints for Poseidon hash function permutation

    for _ in 0..NUM_POS
    {
        // HALF_ROUNDS_FULL full rounds constraint gates
        for j in 0..HALF_ROUNDS_FULL
        {
            gates.push(CircuitGate::<Fq>::create_poseidon(GateWires::wires((i, (i+PERIOD)%N), (i+N, N+((i+PERIOD)%N)), (i+2*N, 2*N+((i+PERIOD)%N))), [c[j][0],c[j][1],c[j][2]], p));
            i+=1;
        }
        // ROUNDS_PARTIAL partial rounds constraint gates
        for j in HALF_ROUNDS_FULL .. HALF_ROUNDS_FULL+ROUNDS_PARTIAL
        {
            gates.push(CircuitGate::<Fq>::create_poseidon(GateWires::wires((i, (i+PERIOD)%N), (i+N, N+((i+PERIOD)%N)), (i+2*N, 2*N+((i+PERIOD)%N))), [c[j][0],c[j][1],c[j][2]], z));
            i+=1;
        }
        // HALF_ROUNDS_FULL full rounds constraint gates
        for j in HALF_ROUNDS_FULL+ROUNDS_PARTIAL .. ROUNDS_FULL+ROUNDS_PARTIAL
        {
            gates.push(CircuitGate::<Fq>::create_poseidon(GateWires::wires((i, (i+PERIOD)%N), (i+N, N+((i+PERIOD)%N)), (i+2*N, 2*N+((i+PERIOD)%N))), [c[j][0],c[j][1],c[j][2]], p));
            i+=1;
        }
        gates.push(CircuitGate::<Fq>::zero(GateWires::wires((i, (i+PERIOD)%N), (i+N, N+((i+PERIOD)%N)), (i+2*N, 2*N+((i+PERIOD)%N)))));
        i+=1;
    }

    let srs = SRS::create(MAX_SIZE);

    let index = Index::<Affine>::create
    (
        ConstraintSystem::<Fq>::create(gates, 0).unwrap(),
        MAX_SIZE,
        oracle::tweedle::fq::params(),
        oracle::tweedle::fp::params(),
        SRSSpec::Use(&srs)
    );
    
    positive(&index);
}

fn positive(index: &Index<Affine>)
{
    let rng = &mut OsRng;

    let params = oracle::tweedle::fq::params();
    let mut sponge = ArithmeticSponge::<Fq>::new();

    let mut batch = Vec::new();
    let group_map = <Affine as CommitmentCurve>::Map::setup();

    println!("{}{:?}", "Circuit size: ".yellow(), N);
    println!("{}{:?}", "Polycommitment chunk size: ".yellow(), MAX_SIZE);
    println!("{}{:?}", "Number oh Poseidon hashes in the circuit: ".yellow(), NUM_POS);
    println!("{}{:?}", "Full rounds: ".yellow(), ROUNDS_FULL);
    println!("{}{:?}", "Sbox alpha: ".yellow(), SPONGE_BOX);
    println!("{}", "Base curve: tweedledum".green());
    println!();
    println!("{}", "Prover zk-proof computation".green());
    let mut start = Instant::now();

    for test in 0..1
    {
        let mut l: Vec<Fq> = Vec::with_capacity(N);
        let mut r: Vec<Fq> = Vec::with_capacity(N);
        let mut o: Vec<Fq> = Vec::with_capacity(N);

        let (x, y, z) = (Fq::rand(rng), Fq::rand(rng), Fq::rand(rng));
        
        //  witness for Poseidon permutation custom constraints
        for _ in 0..NUM_POS
        {
            sponge.state = vec![x, y, z];
            l.push(sponge.state[0]);
            r.push(sponge.state[1]);
            o.push(sponge.state[2]);

            // HALF_ROUNDS_FULL full rounds
            for j in 0..HALF_ROUNDS_FULL
            {
                sponge.full_round(j, &params);
                l.push(sponge.state[0]);
                r.push(sponge.state[1]);
                o.push(sponge.state[2]);
            }
            // ROUNDS_PARTIAL partial rounds
            for j in HALF_ROUNDS_FULL .. HALF_ROUNDS_FULL+ROUNDS_PARTIAL
            {
                sponge.partial_round(j, &params);
                l.push(sponge.state[0]);
                r.push(sponge.state[1]);
                o.push(sponge.state[2]);
            }
            // HALF_ROUNDS_FULL full rounds
            for j in HALF_ROUNDS_FULL+ROUNDS_PARTIAL .. ROUNDS_FULL+ROUNDS_PARTIAL
            {
                sponge.full_round(j, &params);
                l.push(sponge.state[0]);
                r.push(sponge.state[1]);
                o.push(sponge.state[2]);
            }
        }
        let mut witness = l;
        witness.append(&mut r);
        witness.append(&mut o);

        // verify the circuit satisfiability by the computed witness
        assert_eq!(index.cs.verify(&witness), true);

        // add the proof to the batch
        batch.push(ProverProof::create::<DefaultFqSponge<TweedledumParameters>, DefaultFrSponge<Fq>>(
            &group_map, &witness, &index).unwrap());

        print!("{:?}\r", test);
        io::stdout().flush().unwrap();
    }
    println!("{}{:?}", "Execution time: ".yellow(), start.elapsed());

    let verifier_index = index.verifier_index();
    // verify the proofs in batch
    println!("{}", "Verifier zk-proofs verification".green());
    start = Instant::now();
    match ProverProof::verify::<DefaultFqSponge<TweedledumParameters>, DefaultFrSponge<Fq>>(&group_map, &batch, &verifier_index)
    {
        Err(error) => {panic!("Failure verifying the prover's proofs in batch: {}", error)},
        Ok(_) => {println!("{}{:?}", "Execution time: ".yellow(), start.elapsed());}
    }
}
