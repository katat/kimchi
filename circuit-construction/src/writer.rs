use ark_ff::{BigInteger, FftField, PrimeField};
use array_init::array_init;
use kimchi::circuits::{
    gate::{CircuitGate, GateType},
    wires::{Wire, COLUMNS},
};
use oracle::{constants::*, permutation::full_round};
use std::collections::HashMap;

use crate::constants::{Constants, GENERICS, GENERIC_ROW_COEFFS};

#[derive(Hash, Eq, PartialEq, Debug, Clone, Copy)]
pub struct Var<F> {
    pub index: usize,
    pub value: Option<F>,
}

impl<F: Copy> Var<F> {
    pub fn val(&self) -> F {
        self.value.unwrap()
    }
}

pub struct ShiftedScalar<F>(Var<F>);

pub struct GateSpec<F: FftField> {
    pub typ: GateType,
    pub row: [Var<F>; COLUMNS],
    pub c: Vec<F>,
}

pub struct System<F: FftField> {
    pub next_variable: usize,
    // pub equivalence_classes: HashMap<Var, Vec<Position>>,
    pub gates: Vec<GateSpec<F>>,
}

pub struct WitnessGenerator<F> {
    pub rows: Vec<Row<F>>,
}

type Row<V> = [V; COLUMNS];

pub trait Cs<F: FftField + PrimeField> {
    /// In cases where you want to create a free variable in the circuit,
    /// as in the variable is not constrained _yet_
    /// and can be anything that the prover wants.
    /// For example, division can be implemented as:
    ///
    /// ```ignore
    /// let a = sys.constant(5u32.into());
    /// let b = sys.constant(10u32.into());
    /// let c = sys.var(|| {
    ///    b.value * a.value.inverse().unwrap()
    /// });
    /// sys.assert_eq(a * c, b);
    /// ```
    ///
    fn var<G>(&mut self, g: G) -> Var<F>
    where
        G: FnOnce() -> F;

    fn curr_gate_count(&self) -> usize;

    fn endo_scalar<G, N: BigInteger>(&mut self, length: usize, g: G) -> Var<F>
    where
        G: FnOnce() -> N,
    {
        assert_eq!(length % 4, 0);

        self.var(|| {
            let y = g();
            let bits = y.to_bits_le();
            F::from_repr(F::BigInt::from_bits_le(&bits)).unwrap()
        })
    }

    fn scalar<G, Fr: PrimeField>(&mut self, length: usize, g: G) -> ShiftedScalar<F>
    where
        G: FnOnce() -> Fr,
    {
        assert_eq!(length % 5, 0);

        let v = self.var(|| {
            // TODO: No need to recompute this each time.
            let two = Fr::from(2u64);
            let shift = Fr::one() + two.pow(&[length as u64]);

            let x = g();
            // x = 2 y + shift
            // y = (x - shift) / 2
            // TODO: Could cache value of 1/2 to avoid division
            let y = (x - shift) / two;
            let bits = y.into_repr().to_bits_le();
            F::from_repr(F::BigInt::from_bits_le(&bits)).unwrap()
        });
        ShiftedScalar(v)
    }

    /// In circuit mode, adds a gate to the circuit.
    /// In witness generation mode, adds the corresponding row to the witness.
    fn gate(&mut self, g: GateSpec<F>);

    // TODO: Optimize to use permutation argument.
    fn assert_eq(&mut self, x1: Var<F>, x2: Var<F>) {
        // | 0  | 1  | 2 | ...
        // | x1 | x2 | 0 | ...
        let row = array_init(|i| {
            if i == 0 {
                x1
            } else if i == 1 {
                x2
            } else {
                self.var(|| F::zero())
            }
        });

        // constrain `x1 - x2 = 0`
        let mut c = vec![F::zero(); GENERIC_ROW_COEFFS];
        c[0] = F::one();
        c[1] = -F::one();

        self.gate(GateSpec {
            typ: GateType::Generic,
            row,
            c,
        });
    }

    fn constant(&mut self, x: F) -> Var<F> {
        let v = self.var(|| x);

        let mut c = vec![F::zero(); GENERIC_ROW_COEFFS];
        c[0] = F::one();
        c[GENERICS + 1] = -x;

        let row = array_init(|i| if i == 0 { v } else { self.var(|| F::zero()) });

        self.gate(GateSpec {
            typ: GateType::Generic,
            row,
            c,
        });
        v
    }

    // TODO
    fn scale(&mut self, x: F, v: Var<F>) -> Var<F> {
        let xv = self.var(|| v.val() * x);
        let row = {
            let mut row: [_; COLUMNS] = array_init(|_| self.var(|| F::zero()));
            row[0] = v;
            row[1] = xv;
            row
        };

        let mut c = vec![F::zero(); GENERIC_ROW_COEFFS];
        c[0] = x;
        c[1] = -F::one();
        self.gate(GateSpec {
            typ: GateType::Generic,
            row,
            c,
        });
        xv
    }

    fn add_group(
        &mut self,
        zero: Var<F>,
        (x1, y1): (Var<F>, Var<F>),
        (x2, y2): (Var<F>, Var<F>),
    ) -> (Var<F>, Var<F>) {
        let mut same_x_bool = false;
        let same_x = self.var(|| {
            let same_x = x1.val() == x2.val();
            same_x_bool = same_x;
            F::from(same_x as u64)
        });

        let inf = zero;
        let x21_inv = self.var(|| {
            if x1.val() == x2.val() {
                F::zero()
            } else {
                (x2.val() - x1.val()).inverse().unwrap()
            }
        });

        let s = self.var(|| {
            if same_x_bool {
                let x1_squared = x1.val().square();
                (x1_squared.double() + x1_squared).div(y1.val().double())
            } else {
                (y2.val() - y1.val()) * x21_inv.val()
            }
        });

        let inf_z = self.var(|| {
            if y1.val() == y2.val() {
                F::zero()
            } else if same_x_bool {
                (y2.val() - y1.val()).inverse().unwrap()
            } else {
                F::zero()
            }
        });

        let x3 = self.var(|| s.val().square() - (x1.val() + x2.val()));

        let y3 = self.var(|| s.val() * (x1.val() - x3.val()) - y1.val());

        self.gate(GateSpec {
            typ: GateType::CompleteAdd,
            row: [
                x1, y1, x2, y2, x3, y3, inf, same_x, s, inf_z, x21_inv, zero, zero, zero, zero,
            ],
            c: vec![],
        });
        (x3, y3)
    }

    fn double(&mut self, zero: Var<F>, (x1, y1): (Var<F>, Var<F>)) -> (Var<F>, Var<F>) {
        self.add_group(zero, (x1, y1), (x1, y1))
    }

    fn assert_add_group(
        &mut self,
        zero: Var<F>,
        (x1, y1): (Var<F>, Var<F>),
        (x2, y2): (Var<F>, Var<F>),
        (x3, y3): (Var<F>, Var<F>),
    ) {
        let mut same_x_bool = false;
        let same_x = self.var(|| {
            let same_x = x1.val() == x2.val();
            same_x_bool = same_x;
            F::from(same_x as u64)
        });

        let inf = zero;
        let x21_inv = self.var(|| {
            if x1.val() == x2.val() {
                F::zero()
            } else {
                (x2.val() - x1.val()).inverse().unwrap()
            }
        });

        let s = self.var(|| {
            if same_x_bool {
                let x1_squared = x1.val().square();
                (x1_squared.double() + x1_squared).div(y1.val().double())
            } else {
                (y2.val() - y1.val()) * x21_inv.val()
            }
        });

        let inf_z = self.var(|| {
            if y1.val() == y2.val() {
                F::zero()
            } else if same_x_bool {
                (y2.val() - y1.val()).inverse().unwrap()
            } else {
                F::zero()
            }
        });

        self.gate(GateSpec {
            typ: GateType::CompleteAdd,
            row: [
                x1, y1, x2, y2, x3, y3, inf, same_x, s, inf_z, x21_inv, zero, zero, zero, zero,
            ],
            c: vec![],
        });
    }

    // TODO
    fn cond_select(&mut self, b: Var<F>, t: Var<F>, f: Var<F>) -> Var<F> {
        // Could be more efficient. Currently uses three constraints :(
        // delta = t - f
        // res1 = b * delta
        // res = res1 + f

        let delta = self.var(|| t.val() - f.val());
        let res1 = self.var(|| b.val() * delta.val());
        let res = self.var(|| f.val() + res1.val());

        let row1 = {
            let mut r = array_init(|_| self.var(|| F::zero()));
            r[0] = t;
            r[1] = f;
            r[2] = delta;
            r
        };
        let mut c1 = vec![F::zero(); GENERIC_ROW_COEFFS];
        c1[0] = F::one();
        c1[1] = -F::one();
        c1[2] = -F::one();
        self.gate(GateSpec {
            typ: GateType::Generic,
            row: row1,
            c: c1,
        });

        let row2 = {
            let mut r = array_init(|_| self.var(|| F::zero()));
            r[0] = b;
            r[1] = delta;
            r[2] = res1;
            r
        };
        let mut c2 = vec![F::zero(); GENERIC_ROW_COEFFS];
        c2[0] = F::zero();
        c2[1] = F::zero();
        c2[2] = -F::one();
        c2[3] = F::one();

        self.gate(GateSpec {
            typ: GateType::Generic,
            row: row2,
            c: c2,
        });

        let row3 = {
            let mut r = array_init(|_| self.var(|| F::zero()));
            r[0] = res1;
            r[1] = f;
            r[2] = res;
            r
        };
        let mut c3 = vec![F::zero(); GENERIC_ROW_COEFFS];
        c3[0] = F::one();
        c3[1] = F::one();
        c3[2] = -F::one();

        self.gate(GateSpec {
            typ: GateType::Generic,
            row: row3,
            c: c3,
        });

        res
    }

    fn scalar_mul(
        &mut self,
        zero: Var<F>,
        (xt, yt): (Var<F>, Var<F>),
        scalar: ShiftedScalar<F>,
    ) -> (Var<F>, Var<F>) {
        let num_bits = 255;
        let num_row_pairs = num_bits / 5;
        let mut witness: [Vec<F>; COLUMNS] = array_init(|_| vec![]);

        let acc0 = self.add_group(zero, (xt, yt), (xt, yt));

        let _ = self.var(|| {
            witness = array_init(|_| vec![F::zero(); 2 * num_row_pairs]);
            let bits_msb: Vec<bool> = scalar
                .0
                .val()
                .into_repr()
                .to_bits_le()
                .iter()
                .take(num_bits)
                .copied()
                .rev()
                .collect();
            kimchi::circuits::polynomials::varbasemul::witness(
                &mut witness,
                0,
                (xt.val(), yt.val()),
                &bits_msb,
                (acc0.0.val(), acc0.1.val()),
            );
            F::zero()
        });

        let mut res = None;
        for i in 0..num_row_pairs {
            let mut row1 = array_init(|j| self.var(|| witness[j][2 * i]));
            let row2 = array_init(|j| self.var(|| witness[j][2 * i + 1]));

            row1[0] = xt;
            row1[1] = yt;
            if i == 0 {
                row1[2] = acc0.0;
                row1[3] = acc0.1;
                row1[4] = zero;
            }
            if i == num_row_pairs - 1 {
                row1[5] = scalar.0;
                res = Some((row2[0], row2[1]));
            }

            self.gate(GateSpec {
                row: row1,
                typ: GateType::VarBaseMul,
                c: vec![],
            });

            self.gate(GateSpec {
                row: row2,
                typ: GateType::Zero,
                c: vec![],
            })
        }

        res.unwrap()
    }

    fn endo(
        &mut self,
        zero: Var<F>,
        constants: &Constants<F>,
        (xt, yt): (Var<F>, Var<F>),
        scalar: Var<F>,
        length_in_bits: usize,
    ) -> (Var<F>, Var<F>) {
        let bits_per_row = 4;
        let rows = length_in_bits / 4;
        assert_eq!(0, length_in_bits % 4);

        let mut bits_ = vec![];
        let bits: Vec<_> = (0..length_in_bits)
            .map(|i| {
                self.var(|| {
                    if bits_.is_empty() {
                        bits_ = scalar
                            .val()
                            .into_repr()
                            .to_bits_le()
                            .iter()
                            .take(length_in_bits)
                            .copied()
                            .rev()
                            .collect()
                    }
                    F::from(bits_[i] as u64)
                })
            })
            .collect();

        let one = F::one();

        let endo = constants.endo;
        let mut acc = {
            let phip = (self.scale(endo, xt), yt);
            let phip_p = self.add_group(zero, phip, (xt, yt));
            self.double(zero, phip_p)
        };

        let mut n_acc = zero;

        // TODO: Could be more efficient
        for i in 0..rows {
            let b1 = bits[i * bits_per_row];
            let b2 = bits[i * bits_per_row + 1];
            let b3 = bits[i * bits_per_row + 2];
            let b4 = bits[i * bits_per_row + 3];

            let (xp, yp) = acc;

            let xq1 = self.var(|| (one + (endo - one) * b1.val()) * xt.val());
            let yq1 = self.var(|| (b2.val().double() - one) * yt.val());

            let s1 = self.var(|| (yq1.val() - yp.val()) / (xq1.val() - xp.val()));
            let s1_squared = self.var(|| s1.val().square());
            // (2*xp – s1^2 + xq) * ((xp – xr) * s1 + yr + yp) = (xp – xr) * 2*yp
            // => 2 yp / (2*xp – s1^2 + xq) = s1 + (yr + yp) / (xp – xr)
            // => 2 yp / (2*xp – s1^2 + xq) - s1 = (yr + yp) / (xp – xr)
            //
            // s2 := 2 yp / (2*xp – s1^2 + xq) - s1
            //
            // (yr + yp)^2 = (xp – xr)^2 * (s1^2 – xq1 + xr)
            // => (s1^2 – xq1 + xr) = (yr + yp)^2 / (xp – xr)^2
            //
            // => xr = s2^2 - s1^2 + xq
            // => yr = s2 * (xp - xr) - yp
            let s2 = self.var(|| {
                yp.val().double() / (xp.val().double() + xq1.val() - s1_squared.val()) - s1.val()
            });

            // (xr, yr)
            let xr = self.var(|| xq1.val() + s2.val().square() - s1_squared.val());
            let yr = self.var(|| (xp.val() - xr.val()) * s2.val() - yp.val());

            let xq2 = self.var(|| (one + (endo - one) * b3.val()) * xt.val());
            let yq2 = self.var(|| (b4.val().double() - one) * yt.val());
            let s3 = self.var(|| (yq2.val() - yr.val()) / (xq2.val() - xr.val()));
            let s3_squared = self.var(|| s3.val().square());
            let s4 = self.var(|| {
                yr.val().double() / (xr.val().double() + xq2.val() - s3_squared.val()) - s3.val()
            });

            let xs = self.var(|| xq2.val() + s4.val().square() - s3_squared.val());
            let ys = self.var(|| (xr.val() - xs.val()) * s4.val() - yr.val());

            self.gate(GateSpec {
                typ: GateType::EndoMul,
                row: [
                    xt, yt, zero, zero, xp, yp, n_acc, xr, yr, s1, s3, b1, b2, b3, b4,
                ],
                c: vec![],
            });

            acc = (xs, ys);

            n_acc = self.var(|| {
                let mut n_acc = n_acc.val();
                n_acc.double_in_place();
                n_acc += b1.val();
                n_acc.double_in_place();
                n_acc += b2.val();
                n_acc.double_in_place();
                n_acc += b3.val();
                n_acc.double_in_place();
                n_acc += b4.val();
                n_acc
            });
        }
        self.gate(GateSpec {
            typ: GateType::Zero,
            row: [
                zero, zero, zero, zero, acc.0, acc.1, scalar, zero, zero, zero, zero, zero, zero,
                zero, zero,
            ],
            c: vec![],
        });
        acc
    }

    fn assert_pack(&mut self, zero: Var<F>, x: Var<F>, bits_lsb: &[Var<F>]) {
        let crumbs_per_row = 8;
        let bits_per_row = 2 * crumbs_per_row;
        assert_eq!(bits_lsb.len() % bits_per_row, 0);
        let num_rows = bits_lsb.len() / bits_per_row;

        let bits_msb: Vec<_> = bits_lsb.iter().rev().collect();

        let mut a = self.var(|| F::from(2u64));
        let mut b = self.var(|| F::from(2u64));
        let mut n = zero;

        let one = F::one();
        let neg_one = -one;

        for (i, row_bits) in bits_msb[..].chunks(bits_per_row).enumerate() {
            let mut row: [Var<F>; COLUMNS] = array_init(|_| self.var(|| F::zero()));
            row[0] = n;
            row[2] = a;
            row[3] = b;

            for (j, crumb_bits) in row_bits.chunks(2).enumerate() {
                let b0 = crumb_bits[1];
                let b1 = crumb_bits[0];

                let crumb = self.var(|| b0.val() + b1.val().double());
                row[6 + j] = crumb;

                a = self.var(|| {
                    let x = a.val().double();
                    if b1.val().is_zero() {
                        x
                    } else {
                        x + if b0.val().is_one() { one } else { neg_one }
                    }
                });

                b = self.var(|| {
                    let x = b.val().double();
                    if b1.val().is_zero() {
                        x + if b0.val().is_one() { one } else { neg_one }
                    } else {
                        x
                    }
                });

                n = self.var(|| n.val().double().double() + crumb.val());
            }

            row[1] = if i == num_rows - 1 { x } else { n };
            row[4] = a;
            row[5] = b;

            row[14] = self.var(|| F::zero());
        }
    }

    fn poseidon(&mut self, constants: &Constants<F>, input: Vec<Var<F>>) -> Vec<Var<F>> {
        use kimchi::circuits::polynomials::poseidon::*;

        let params = &constants.poseidon;
        let rc = &params.round_constants;
        let width = PlonkSpongeConstantsKimchi::SPONGE_WIDTH;

        let mut states = vec![input];

        for row in 0..POS_ROWS_PER_HASH {
            let offset = row * ROUNDS_PER_ROW;

            for i in 0..ROUNDS_PER_ROW {
                let mut s: Option<Vec<F>> = None;
                states.push(
                    (0..3)
                        .map(|col| {
                            self.var(|| {
                                match &s {
                                    Some(s) => s[col],
                                    None => {
                                        // Do one full round on the previous value
                                        let mut acc = states[states.len() - 1]
                                            .iter()
                                            .map(|x| x.val())
                                            .collect();
                                        full_round::<F, PlonkSpongeConstantsKimchi>(
                                            params,
                                            &mut acc,
                                            offset + i,
                                        );
                                        let res = acc[col];
                                        s = Some(acc);
                                        res
                                    }
                                }
                            })
                        })
                        .collect(),
                );
            }

            self.gate(GateSpec {
                typ: kimchi::circuits::gate::GateType::Poseidon,
                c: (0..15)
                    .map(|i| rc[offset + (i / width)][i % width])
                    .collect(),
                row: [
                    states[offset][0],
                    states[offset][1],
                    states[offset][2],
                    states[offset + 4][0],
                    states[offset + 4][1],
                    states[offset + 4][2],
                    states[offset + 1][0],
                    states[offset + 1][1],
                    states[offset + 1][2],
                    states[offset + 2][0],
                    states[offset + 2][1],
                    states[offset + 2][2],
                    states[offset + 3][0],
                    states[offset + 3][1],
                    states[offset + 3][2],
                ],
            });
        }

        let mut final_row = array_init(|_| self.var(|| F::zero()));
        final_row[0] = states[states.len() - 1][0];
        final_row[1] = states[states.len() - 1][1];
        final_row[2] = states[states.len() - 1][2];
        self.gate(GateSpec {
            typ: kimchi::circuits::gate::GateType::Zero,
            c: vec![],
            row: final_row,
        });

        states.pop().unwrap()
    }
}

impl<F: FftField + PrimeField> Cs<F> for WitnessGenerator<F> {
    fn var<G>(&mut self, g: G) -> Var<F>
    where
        G: FnOnce() -> F,
    {
        Var {
            index: 0,
            value: Some(g()),
        }
    }

    fn curr_gate_count(&self) -> usize {
        self.rows.len()
    }

    fn gate(&mut self, g: GateSpec<F>) {
        self.rows.push(array_init(|i| g.row[i].value.unwrap()))
    }
}

impl<F: FftField> WitnessGenerator<F> {
    /// Returns the columns of the witness.
    pub fn columns(&self) -> [Vec<F>; COLUMNS] {
        array_init(|col| self.rows.iter().map(|row| row[col]).collect())
    }
}

impl<F: FftField + PrimeField> Cs<F> for System<F> {
    fn var<G>(&mut self, _: G) -> Var<F> {
        let v = self.next_variable;
        self.next_variable += 1;
        Var {
            index: v,
            value: None,
        }
    }

    fn curr_gate_count(&self) -> usize {
        self.gates.len()
    }

    fn gate(&mut self, g: GateSpec<F>) {
        self.gates.push(g);
    }
}

impl<F: FftField> System<F> {
    /// Compiles our intermediate representation into a circuit.
    pub fn gates(&self) -> Vec<CircuitGate<F>> {
        let mut first_cell: HashMap<usize, Wire> = HashMap::new();
        let mut most_recent_cell: HashMap<usize, Wire> = HashMap::new();
        let mut gates = vec![];

        // convert GateSpec into CircuitGate
        for (row, gate) in self.gates.iter().enumerate() {
            // while tracking the wiring
            let wires = array_init(|col| -> Wire {
                let var = gate.row[col].index;
                let curr = Wire { row, col };

                // wire this cell to the previous one
                match most_recent_cell.insert(var, curr) {
                    Some(w) => w,
                    // unless it is the first cell,
                    // in which case we just save it for the very end
                    // (to complete the cycle)
                    None => {
                        first_cell.insert(var, curr);
                        curr
                    }
                }
            });

            let g = CircuitGate {
                typ: gate.typ,
                coeffs: gate.c.clone(),
                wires,
            };
            gates.push(g);
        }

        // finish the permutation cycle
        for (var, first) in first_cell.iter() {
            let last = *most_recent_cell.get(var).unwrap();
            gates[first.row].wires[first.col] = last;
        }

        gates
    }
}
