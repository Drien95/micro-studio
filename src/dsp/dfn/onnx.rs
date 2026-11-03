//! Lecteur minimal de fichiers ONNX : extrait les tenseurs de poids (les
//! « initializers » et les nœuds `Constant`) d'un `ModelProto`, sans rien
//! interpréter du graphe. Le format est du Protocol Buffers ; seuls les
//! champs utiles sont décodés, les autres sont sautés.
//!
//! Champs lus (numéros de onnx.proto3) :
//! - `ModelProto.graph` = 7
//! - `GraphProto.node` = 1, `GraphProto.initializer` = 5
//! - `NodeProto.output` = 2, `NodeProto.op_type` = 4, `NodeProto.attribute` = 5
//! - `AttributeProto.t` = 5
//! - `TensorProto.dims` = 1, `data_type` = 2, `float_data` = 4, `name` = 8, `raw_data` = 9

use std::collections::HashMap;

/// Tenseur de poids en `f32`, rangé ligne par ligne (le dernier axe varie le plus vite).
#[derive(Clone, Debug, PartialEq)]
pub struct Tensor {
    pub dims: Vec<usize>,
    pub data: Vec<f32>,
}

/// Erreur de lecture : fichier tronqué, type inattendu…
#[derive(Debug, PartialEq, Eq)]
pub struct Error(pub &'static str);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "modèle ONNX illisible : {}", self.0)
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// `TensorProto.DataType.FLOAT`.
const FLOAT: u64 = 1;

/// Curseur sur un message protobuf.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

/// Valeur d'un champ : entier (varint ou fixe) ou bloc de longueur connue.
enum Value<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = *self.buf.get(self.pos).ok_or(Error("entier tronqué"))?;
            self.pos += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Error("entier trop long"))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).ok_or(Error("bloc tronqué"))?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Champ suivant : `(numéro, valeur)`, ou `None` en fin de message.
    fn field(&mut self) -> Result<Option<(u64, Value<'a>)>> {
        if self.pos >= self.buf.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let value = match key & 7 {
            0 => Value::Int(self.varint()?),
            1 => Value::Int(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| Error("fixe 64"))?)),
            2 => {
                let n = usize::try_from(self.varint()?).map_err(|_| Error("longueur"))?;
                Value::Bytes(self.take(n)?)
            }
            5 => Value::Int(u64::from(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| Error("fixe 32"))?))),
            _ => return Err(Error("type de champ inconnu")),
        };
        Ok(Some((key >> 3, value)))
    }
}

/// Décode un `TensorProto` en `(nom, tenseur)`.
fn tensor(buf: &[u8]) -> Result<(String, Tensor)> {
    let mut r = Reader::new(buf);
    let (mut dims, mut name, mut data, mut dtype) = (Vec::new(), String::new(), Vec::new(), FLOAT);
    while let Some((num, v)) = r.field()? {
        match (num, v) {
            (1, Value::Int(d)) => dims.push(usize::try_from(d).map_err(|_| Error("dimension"))?),
            (1, Value::Bytes(packed)) => {
                let mut p = Reader::new(packed);
                while p.pos < packed.len() {
                    dims.push(usize::try_from(p.varint()?).map_err(|_| Error("dimension"))?);
                }
            }
            (2, Value::Int(t)) => dtype = t,
            (4, Value::Bytes(packed)) => {
                data.extend(packed.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)));
            }
            (4, Value::Int(bits)) => data.push(f32::from_bits(u32::try_from(bits).map_err(|_| Error("flottant"))?)),
            (8, Value::Bytes(n)) => name = String::from_utf8_lossy(n).into_owned(),
            (9, Value::Bytes(raw)) => {
                data.extend(raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)));
            }
            _ => {}
        }
    }
    if dtype != FLOAT {
        // Entiers (formes, indices) : sans intérêt pour les poids.
        data.clear();
    }
    Ok((name, Tensor { dims, data }))
}

/// Tous les tenseurs `f32` d'un modèle ONNX, par nom. Les sorties des nœuds
/// `Constant` sont rangées sous le nom de leur sortie.
pub fn weights(model: &[u8]) -> Result<HashMap<String, Tensor>> {
    let mut out = HashMap::new();
    let mut r = Reader::new(model);
    while let Some((num, v)) = r.field()? {
        let (7, Value::Bytes(graph)) = (num, v) else { continue };
        let mut g = Reader::new(graph);
        while let Some((num, v)) = g.field()? {
            match (num, v) {
                (5, Value::Bytes(t)) => {
                    let (name, t) = tensor(t)?;
                    out.insert(name, t);
                }
                (1, Value::Bytes(node)) => {
                    if let Some((name, t)) = constant(node)? {
                        out.insert(name, t);
                    }
                }
                _ => {}
            }
        }
    }
    out.retain(|_, t| !t.data.is_empty() && t.data.len() == t.dims.iter().product::<usize>());
    Ok(out)
}

/// Tenseur porté par un nœud `Constant`, sous le nom de sa sortie.
fn constant(node: &[u8]) -> Result<Option<(String, Tensor)>> {
    let mut r = Reader::new(node);
    let (mut output, mut is_const, mut value) = (None, false, None);
    while let Some((num, v)) = r.field()? {
        match (num, v) {
            (2, Value::Bytes(o)) => output = Some(String::from_utf8_lossy(o).into_owned()),
            (4, Value::Bytes(op)) => is_const = op == b"Constant",
            (5, Value::Bytes(attr)) => {
                let mut a = Reader::new(attr);
                while let Some((num, v)) = a.field()? {
                    if let (5, Value::Bytes(t)) = (num, v) {
                        value = Some(tensor(t)?.1);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(match (is_const, output, value) {
        (true, Some(o), Some(t)) => Some((o, t)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode un champ protobuf (numéro, type 2) pour fabriquer des messages de test.
    fn bytes_field(num: u64, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![u8::try_from((num << 3) | 2).expect("petit numéro")];
        let mut n = payload.len();
        loop {
            let b = u8::try_from(n & 0x7f).expect("7 bits");
            n >>= 7;
            if n == 0 {
                v.push(b);
                break;
            }
            v.push(b | 0x80);
        }
        v.extend_from_slice(payload);
        v
    }

    fn tensor_proto(name: &str, dims: &[u8], values: &[f32]) -> Vec<u8> {
        let mut t = Vec::new();
        for &d in dims {
            t.extend([1 << 3, d]); // dims, varint
        }
        t.extend([2 << 3, 1]); // data_type = FLOAT
        t.extend(bytes_field(8, name.as_bytes()));
        let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        t.extend(bytes_field(9, &raw));
        t
    }

    #[test]
    fn reads_initializers_and_constants() {
        let init = tensor_proto("w", &[2, 2], &[1.0, 2.0, 3.0, -4.5]);
        let mut attr = bytes_field(1, b"value");
        attr.extend(bytes_field(5, &tensor_proto("", &[3], &[0.5, 0.25, 0.125])));
        let mut node = bytes_field(2, b"gru_w");
        node.extend(bytes_field(4, b"Constant"));
        node.extend(bytes_field(5, &attr));
        let mut graph = bytes_field(5, &init);
        graph.extend(bytes_field(1, &node));
        let mut model = vec![1 << 3, 7]; // ir_version, ignoré
        model.extend(bytes_field(7, &graph));

        let w = weights(&model).expect("lecture");
        assert_eq!(w["w"], Tensor { dims: vec![2, 2], data: vec![1.0, 2.0, 3.0, -4.5] });
        assert_eq!(w["gru_w"].data, vec![0.5, 0.25, 0.125]);
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn truncated_files_are_rejected() {
        let model = bytes_field(7, &bytes_field(5, &tensor_proto("w", &[1], &[1.0])));
        for cut in [1, 3, model.len() - 1] {
            assert!(weights(&model[..cut]).is_err(), "coupé à {cut}");
        }
        assert!(weights(&[0xff; 12]).is_err(), "entier trop long");
        assert!(weights(&[(1 << 3) | 3]).is_err(), "type de champ inconnu");
        assert_eq!(Error("x").to_string(), "modèle ONNX illisible : x");
        assert!(weights(&[]).expect("vide").is_empty());
    }

    #[test]
    fn real_model_has_the_expected_weights() {
        let w = weights(include_bytes!("../../../models/DeepFilterNet3/enc.onnx")).expect("enc.onnx");
        assert_eq!(w["onnx::Conv_282"].dims, vec![64, 1, 3, 3]);
        assert_eq!(w["/emb_gru/Constant_3_output_0"].dims, vec![1, 768, 256]);
        assert_eq!(w["df_fc_emb.0.weight"].dims, vec![32, 96, 16]);
    }
}
