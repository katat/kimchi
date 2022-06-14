//! Implements a tool to visualize a circuit as an HTML page.

use ark_ff::PrimeField;
use commitment_dlog::commitment::CommitmentCurve;
use kimchi::{
    circuits::{
        argument::Argument,
        polynomials::{
            chacha::{ChaCha0, ChaCha1, ChaCha2, ChaChaFinal},
            complete_add::CompleteAdd,
            endomul_scalar::EndomulScalar,
            endosclmul::EndosclMul,
            poseidon::Poseidon,
            varbasemul::VarbaseMul,
        },
    },
    prover_index::ProverIndex,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    fmt::Display,
    fs::{self, File},
    io::Write,
    path::Path,
};
use tinytemplate::TinyTemplate;

pub mod witness;

pub use witness::Witness;

/// Contains variable used in the template
#[derive(Serialize)]
struct Context {
    js: String,
    data: String,
}

/// Allows us to quickly implement a LaTeX encoder for each gate
trait LaTeX<F>: Argument<F>
where
    F: PrimeField,
{
    fn latex() -> Vec<Vec<String>> {
        Self::constraints().iter().map(|c| c.latex_str()).collect()
    }
}

/// Implement [LaTeX] for all gates
impl<T, F> LaTeX<F> for T
where
    T: Argument<F>,
    F: PrimeField + Display,
{
}

///
pub fn latex_constraints<G>() -> HashMap<&'static str, Vec<Vec<String>>>
where
    G: CommitmentCurve,
{
    let mut map = HashMap::new();
    map.insert("Poseidon", Poseidon::<G::ScalarField>::latex());
    map.insert("CompleteAdd", CompleteAdd::<G::ScalarField>::latex());
    map.insert("VarBaseMul", VarbaseMul::<G::ScalarField>::latex());
    map.insert("EndoMul", EndosclMul::<G::ScalarField>::latex());
    map.insert("EndoMulScalar", EndomulScalar::<G::ScalarField>::latex());
    map.insert("ChaCha0", ChaCha0::<G::ScalarField>::latex());
    map.insert("ChaCha1", ChaCha1::<G::ScalarField>::latex());
    map.insert("ChaCha2", ChaCha2::<G::ScalarField>::latex());
    map.insert("ChaChaFinal", ChaChaFinal::<G::ScalarField>::latex());
    map
}

/// Produces a `circuit.html` in the current folder.
pub fn visu<G>(index: &ProverIndex<G>, witness: Option<Witness<G::ScalarField>>)
where
    G: CommitmentCurve,
{
    // serialize index
    let index = serde_json::to_string(index).expect("couldn't serialize index");
    let mut data = format!("const index = {index};");

    // serialize witness
    if let Some(witness) = witness {
        let witness = serde_json::to_string(&witness).expect("couldn't serialize witness");
        data.push_str(&format!("const witness = {witness};"));
    } else {
        data.push_str("const witness = null;");
    }

    // serialize constraints
    let constraints = latex_constraints::<G>();
    let constraints = serde_json::to_string(&constraints).expect("couldn't serialize constraints");
    data.push_str(&format!("const constraints = {constraints};"));

    // create template
    let template_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assets/template.html");
    let template = fs::read_to_string(&template_path).unwrap_or_else(|e| {
        format!(
            "could not read template file {}: {e}",
            template_path.display()
        )
    });

    let mut tt = TinyTemplate::new();
    tt.set_default_formatter(&tinytemplate::format_unescaped);
    tt.add_template("circuit", &template)
        .expect("could not create template");

    // render
    let html_output = std::env::current_dir()
        .expect("no current directory?")
        .join("circuit.html");

    let js_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assets/script.js");
    let js = fs::read_to_string(&js_path)
        .unwrap_or_else(|e| format!("could not read js file {}: {e}", js_path.display()));

    let context = Context { js, data };

    let rendered = tt
        .render("circuit", &context)
        .unwrap_or_else(|e| panic!("template file can't be rendered: {}", e));

    let mut file = File::create(html_output).unwrap_or_else(|e| panic!("{e}"));
    write!(&mut file, "{rendered}").expect("couldn't write the file on disk");
}
