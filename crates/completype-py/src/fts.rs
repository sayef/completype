//! Experimental bindings for `completype-fts`, the `completype.fts` module.

use std::io;
use std::ops::Bound as Limit;
use std::path::PathBuf;
use std::sync::Mutex;

use completype_fts as fts;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyString, PyTuple};

use crate::{invalid, py_repr, raise, strings};

fn to_py_err(e: io::Error) -> PyErr {
    let name = match e.kind() {
        io::ErrorKind::InvalidInput => "InvalidInputError",
        io::ErrorKind::InvalidData => "CorruptionError",
        _ => "StorageError",
    };
    raise(name, e.to_string())
}

fn language(name: &str) -> PyResult<fts::Language> {
    fts::Language::from_name(&name.to_lowercase()).ok_or_else(|| {
        let names: Vec<&str> = fts::Language::ALL.iter().map(|l| l.name()).collect();
        invalid(format!(
            "unknown language {name:?}, expected one of {}",
            names.join(", ")
        ))
    })
}

fn snowball(name: &str) -> PyResult<fts::StopWords> {
    let language = language(name)?;
    fts::StopWords::snowball(language).ok_or_else(|| {
        invalid(format!(
            "there is no Snowball stop word list for {language}"
        ))
    })
}

/// `(key, value)` pairs of a mapping.
fn items<'py>(value: &Bound<'py, PyAny>, what: &str) -> PyResult<Vec<(String, Bound<'py, PyAny>)>> {
    let items = match value.cast::<PyDict>() {
        Ok(dict) => dict.items().into_any(),
        Err(_) => value
            .call_method0("items")
            .map_err(|_| invalid(format!("{what} must be a mapping")))?,
    };
    items
        .try_iter()?
        .map(|pair| {
            pair?
                .extract()
                .map_err(|_| invalid(format!("{what} must map field names to values")))
        })
        .collect()
}

/// One value or a list of them; `None` gives none.
fn each<'py>(value: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, PyAny>>> {
    if value.is_none() {
        Ok(Vec::new())
    } else if value.is_instance_of::<PyString>()
        || value.is_instance_of::<PyBool>()
        || !value.hasattr("__iter__")?
    {
        Ok(vec![value.clone()])
    } else {
        value.try_iter()?.collect()
    }
}

fn string(value: &Bound<'_, PyAny>, field: &str, kind: &str) -> PyResult<String> {
    value
        .cast::<PyString>()
        .map(|s| s.to_string())
        .map_err(|_| invalid(format!("{kind} field {field:?} must hold strings")))
}

fn number(value: &Bound<'_, PyAny>, field: &str) -> PyResult<fts::Number> {
    if value.is_instance_of::<PyBool>() {
        return Err(invalid(format!(
            "numeric field {field:?} must hold numbers, not bools"
        )));
    }
    if !value.is_instance_of::<PyFloat>() {
        if let Ok(v) = value.extract::<i64>() {
            return Ok(fts::Number::Int(v));
        }
    }
    value
        .extract::<f64>()
        .map(fts::Number::Float)
        .map_err(|_| invalid(format!("numeric field {field:?} must hold ints or floats")))
}

fn payload(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = value.cast::<PyBytes>() {
        Ok(bytes.as_bytes().to_vec())
    } else if let Ok(bytes) = value.cast::<PyByteArray>() {
        Ok(bytes.to_vec())
    } else if let Ok(text) = value.cast::<PyString>() {
        Ok(text.to_str()?.as_bytes().to_vec())
    } else {
        let kwargs = PyDict::new(py);
        kwargs.set_item("ensure_ascii", false)?;
        kwargs.set_item("separators", (",", ":"))?;
        let json: String = py
            .import("json")?
            .call_method("dumps", (value,), Some(&kwargs))?
            .extract()?;
        Ok(json.into_bytes())
    }
}

fn document(
    py: Python<'_>,
    text: Option<&Bound<'_, PyAny>>,
    keywords: Option<&Bound<'_, PyAny>>,
    numbers: Option<&Bound<'_, PyAny>>,
    stored: Option<&Bound<'_, PyAny>>,
) -> PyResult<fts::Document> {
    let mut doc = fts::Document::new();
    match text {
        None => {}
        Some(t) if t.is_instance_of::<PyString>() => {
            doc = doc.text(fts::DEFAULT_FIELD, string(t, fts::DEFAULT_FIELD, "text")?);
        }
        Some(t) => {
            for (field, value) in items(t, "text")? {
                for v in each(&value)? {
                    doc = doc.text(field.as_str(), string(&v, &field, "text")?);
                }
            }
        }
    }
    for (field, value) in keywords
        .map(|k| items(k, "keywords"))
        .transpose()?
        .unwrap_or_default()
    {
        for v in each(&value)? {
            doc = doc.keyword(field.as_str(), string(&v, &field, "keyword")?);
        }
    }
    for (field, value) in numbers
        .map(|n| items(n, "numbers"))
        .transpose()?
        .unwrap_or_default()
    {
        for v in each(&value)? {
            doc = doc.number(field.as_str(), number(&v, &field)?);
        }
    }
    if let Some(stored) = stored.filter(|s| !s.is_none()) {
        doc = doc.stored(payload(py, stored)?);
    }
    Ok(doc)
}

/// A full-text index in a directory. Experimental: the API may change in any release.
#[pyclass(frozen, module = "completype.fts")]
struct Index {
    index: fts::Index,
    path: PathBuf,
}

#[pymethods]
impl Index {
    /// Opens the index in `path`, creating an empty one if there is none.
    #[staticmethod]
    #[pyo3(signature = (path, *, language = None, keep_accents = None, stop_words = None, reordering = "on_merge", first_letter_typos = false))]
    fn open(
        py: Python<'_>,
        path: PathBuf,
        language: Option<&str>,
        keep_accents: Option<bool>,
        stop_words: Option<&Bound<'_, PyAny>>,
        reordering: &str,
        first_letter_typos: bool,
    ) -> PyResult<Self> {
        let reordering = match reordering {
            "never" => fts::Reordering::Never,
            "on_merge" => fts::Reordering::OnMerge,
            "always" => fts::Reordering::Always,
            other => {
                return Err(invalid(format!(
                    "reordering must be \"never\", \"on_merge\" or \"always\", not {other:?}"
                )))
            }
        };
        let analyzer = if language.is_some() || keep_accents.is_some() {
            let mut analyzer = fts::Analyzer::default();
            if keep_accents == Some(true) {
                analyzer = analyzer.keep_accents();
            }
            if let Some(name) = language {
                analyzer = analyzer.stem(self::language(name)?);
            }
            Some(analyzer)
        } else {
            None
        };
        let stop_words = match stop_words {
            Some(s) if s.is_instance_of::<PyString>() => Some(snowball(&s.extract::<String>()?)?),
            Some(s) if !s.is_none() => Some(fts::StopWords::new(strings(s, "stop_words")?)),
            _ => None,
        };
        let mut index = py
            .detach(|| fts::Index::open(&path))
            .map_err(to_py_err)?
            .with_reordering(reordering)
            .with_first_letter_typos(first_letter_typos);
        if let Some(analyzer) = analyzer {
            index = index.with_analyzer(analyzer);
        }
        if let Some(stop_words) = stop_words {
            index = index.with_stop_words(stop_words);
        }
        Ok(Self { index, path })
    }

    #[getter]
    fn path(&self) -> &PathBuf {
        &self.path
    }

    /// The stemming language, or `None` when words are not stemmed.
    #[getter]
    fn language(&self) -> Option<&'static str> {
        self.index.analyzer().language().map(fts::Language::name)
    }

    /// The only writer; raises `StorageError` while another holds the lock.
    fn writer(&self, py: Python<'_>) -> PyResult<Writer> {
        let writer = py.detach(|| self.index.writer()).map_err(to_py_err)?;
        Ok(Writer(Mutex::new(Some(writer))))
    }

    /// A snapshot of the committed documents.
    fn reader(&self, py: Python<'_>) -> PyResult<Reader> {
        Ok(Reader(
            py.detach(|| self.index.reader()).map_err(to_py_err)?,
        ))
    }

    /// `Reader.search` on a snapshot of the latest commit.
    #[pyo3(signature = (query, limit = 10, *, fields = None, filter = None, typos = true, stored = true))]
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        py: Python<'_>,
        query: &str,
        limit: usize,
        fields: Option<&Bound<'_, PyAny>>,
        filter: Option<&Bound<'_, Filter>>,
        typos: bool,
        stored: bool,
    ) -> PyResult<Vec<Hit>> {
        self.reader(py)?
            .search(py, query, limit, fields, filter, typos, stored)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let language = match self.language() {
            Some(l) => py_repr(py, l)?,
            None => "None".to_owned(),
        };
        Ok(format!(
            "Index(path={}, language={language})",
            py_repr(py, &self.path.to_string_lossy())?
        ))
    }
}

/// Adds and deletes documents; a commit makes them visible to new readers. As a context manager
/// it commits on success and discards the changes on an exception, then releases the lock.
#[pyclass(frozen, module = "completype.fts")]
struct Writer(Mutex<Option<fts::Writer>>);

impl Writer {
    /// Runs `f` on the open writer without the GIL.
    fn with<T: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&mut fts::Writer) -> io::Result<T> + Send,
    ) -> PyResult<T> {
        py.detach(|| {
            let mut writer = self.0.lock().unwrap_or_else(|e| e.into_inner());
            writer.as_mut().map(f)
        })
        .ok_or_else(|| PyValueError::new_err("the writer is closed"))?
        .map_err(to_py_err)
    }
}

#[pymethods]
impl Writer {
    /// Adds a document, replacing any with the same id. `text` is a string for the field `text`
    /// or a mapping of field to text; `stored` is bytes, a string, or a value saved as JSON.
    #[pyo3(signature = (id, text = None, *, keywords = None, numbers = None, stored = None))]
    fn add(
        &self,
        py: Python<'_>,
        id: &str,
        text: Option<&Bound<'_, PyAny>>,
        keywords: Option<&Bound<'_, PyAny>>,
        numbers: Option<&Bound<'_, PyAny>>,
        stored: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let doc = document(py, text, keywords, numbers, stored)?;
        self.with(py, |w| {
            w.add_document(id, &doc);
            Ok(())
        })
    }

    /// Deletes the document `id`, committed or not; an unknown id is ignored.
    fn delete(&self, py: Python<'_>, id: &str) -> PyResult<()> {
        self.with(py, |w| {
            w.delete(id);
            Ok(())
        })
    }

    /// Makes the changes so far visible to new readers, atomically.
    fn commit(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, fts::Writer::commit)
    }

    /// Merges every committed segment into one, dropping deleted documents.
    fn merge(&self, py: Python<'_>) -> PyResult<()> {
        self.with(py, fts::Writer::merge)
    }

    /// Discards uncommitted changes and releases the lock.
    fn close(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    fn __enter__(slf: Bound<'_, Self>) -> Bound<'_, Self> {
        slf
    }

    #[pyo3(signature = (exc_type, _exc_value, _traceback))]
    fn __exit__(
        &self,
        py: Python<'_>,
        exc_type: Option<&Bound<'_, PyAny>>,
        _exc_value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let open = self.0.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        let committed = if exc_type.is_none() && open {
            self.commit(py)
        } else {
            Ok(())
        };
        self.close();
        committed.map(|()| false)
    }
}

/// A consistent view of the index at one commit.
#[pyclass(frozen, module = "completype.fts")]
struct Reader(fts::Reader);

#[pymethods]
impl Reader {
    /// The best `limit` documents for `query` by BM25. `fields` limits the search to some text
    /// fields, with a boost each if a mapping; `typos=False` skips spelling corrections.
    #[pyo3(signature = (query, limit = 10, *, fields = None, filter = None, typos = true, stored = true))]
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        py: Python<'_>,
        query: &str,
        limit: usize,
        fields: Option<&Bound<'_, PyAny>>,
        filter: Option<&Bound<'_, Filter>>,
        typos: bool,
        stored: bool,
    ) -> PyResult<Vec<Hit>> {
        let mut q = fts::Query::new(query).limit(limit);
        match fields {
            Some(f) if f.is_none() => {}
            Some(f) if f.cast::<PyDict>().is_ok() || f.hasattr("items")? => {
                for (field, boost) in items(f, "fields")? {
                    let boost: f32 = boost
                        .extract()
                        .map_err(|_| invalid("field boosts must be numbers"))?;
                    q = q.field(field, boost);
                }
            }
            Some(f) => {
                for field in strings(f, "fields")? {
                    q = q.field(field, 1.0);
                }
            }
            None => {}
        }
        if let Some(filter) = filter {
            q = q.filter(filter.get().0.clone());
        }
        if !typos {
            q = q.exact();
        }
        let (hits, payloads) = py.detach(|| {
            let hits = self.0.query(&q);
            let payloads = if stored {
                self.0.documents(&hits)
            } else {
                vec![None; hits.len()]
            };
            (hits, payloads)
        });
        Ok(hits
            .into_iter()
            .zip(payloads)
            .map(|(hit, payload)| Hit {
                id: hit.id,
                score: hit.score,
                stored: payload.map(|p| PyBytes::new(py, &p).unbind()),
            })
            .collect())
    }

    /// The stored payload of document `id`, or `None` if it has none or does not exist.
    fn get<'py>(&self, py: Python<'py>, id: &str) -> Option<Bound<'py, PyBytes>> {
        py.detach(|| self.0.document(id))
            .map(|p| PyBytes::new(py, &p))
    }

    #[getter]
    fn segment_count(&self) -> usize {
        self.0.segment_count()
    }

    fn __len__(&self) -> usize {
        self.0.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "Reader(documents={}, segments={})",
            self.0.len(),
            self.0.segment_count()
        )
    }
}

/// One search result.
#[pyclass(frozen, get_all, module = "completype.fts")]
struct Hit {
    id: String,
    score: f32,
    stored: Option<Py<PyBytes>>,
}

#[pymethods]
impl Hit {
    #[classattr]
    fn __match_args__() -> (&'static str, &'static str, &'static str) {
        ("id", "score", "stored")
    }

    /// The stored payload parsed as JSON, or `None` without one.
    fn json<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        self.stored
            .as_ref()
            .map(|p| py.import("json")?.call_method1("loads", (p.bind(py),)))
            .transpose()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!(
            "Hit(id={}, score={:.4})",
            py_repr(py, &self.id)?,
            self.score
        ))
    }
}

/// Which documents a search may return. Combine with `&`, `|` and `~`.
#[pyclass(frozen, module = "completype.fts")]
struct Filter(fts::Filter);

fn bound(value: Option<&Bound<'_, PyAny>>, field: &str) -> PyResult<Option<fts::Number>> {
    value
        .filter(|v| !v.is_none())
        .map(|v| number(v, field))
        .transpose()
}

#[pymethods]
impl Filter {
    /// Documents whose keyword field `field` holds `value`.
    #[staticmethod]
    fn keyword(field: String, value: String) -> Self {
        Self(fts::Filter::keyword(field, value))
    }

    /// Documents whose keyword field `field` holds any of `values`.
    #[staticmethod]
    fn keyword_in(field: String, values: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self(fts::Filter::keyword_in(
            field,
            strings(values, "values")?,
        )))
    }

    /// Documents whose numeric field `field` holds a value within the bounds; none means any.
    #[staticmethod]
    #[pyo3(signature = (field, *, gt = None, ge = None, lt = None, le = None))]
    fn range(
        field: String,
        gt: Option<&Bound<'_, PyAny>>,
        ge: Option<&Bound<'_, PyAny>>,
        lt: Option<&Bound<'_, PyAny>>,
        le: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let lower = match (bound(gt, &field)?, bound(ge, &field)?) {
            (Some(_), Some(_)) => return Err(invalid("pass gt or ge, not both")),
            (Some(v), None) => Limit::Excluded(v),
            (None, Some(v)) => Limit::Included(v),
            (None, None) => Limit::Unbounded,
        };
        let upper = match (bound(lt, &field)?, bound(le, &field)?) {
            (Some(_), Some(_)) => return Err(invalid("pass lt or le, not both")),
            (Some(v), None) => Limit::Excluded(v),
            (None, Some(v)) => Limit::Included(v),
            (None, None) => Limit::Unbounded,
        };
        Ok(Self(fts::Filter::Range {
            field,
            lower,
            upper,
        }))
    }

    /// Documents whose numeric field `field` holds `value`.
    #[staticmethod]
    fn equals(field: String, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        let value = number(value, &field)?;
        Ok(Self(fts::Filter::equals(field, value)))
    }

    fn __and__(&self, other: &Bound<'_, Filter>) -> Self {
        Self(self.0.clone().and(other.get().0.clone()))
    }

    fn __or__(&self, other: &Bound<'_, Filter>) -> Self {
        Self(self.0.clone().or(other.get().0.clone()))
    }

    fn __invert__(&self) -> Self {
        Self(self.0.clone().not())
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other.cast::<Filter>().is_ok_and(|o| o.get().0 == self.0)
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        filter_repr(py, &self.0)
    }
}

fn number_repr(n: &fts::Number) -> String {
    match n {
        fts::Number::Int(v) => v.to_string(),
        fts::Number::Float(v) => format!("{v:?}"),
    }
}

fn filter_repr(py: Python<'_>, filter: &fts::Filter) -> PyResult<String> {
    let join = |filters: &[fts::Filter], op: &str| -> PyResult<String> {
        let parts = filters
            .iter()
            .map(|f| filter_repr(py, f))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(format!("({})", parts.join(op)))
    };
    Ok(match filter {
        fts::Filter::Keyword { field, values } if values.len() == 1 => format!(
            "Filter.keyword({}, {})",
            py_repr(py, field)?,
            py_repr(py, &values[0])?
        ),
        fts::Filter::Keyword { field, values } => {
            let values = values
                .iter()
                .map(|v| py_repr(py, v))
                .collect::<PyResult<Vec<_>>>()?;
            format!(
                "Filter.keyword_in({}, [{}])",
                py_repr(py, field)?,
                values.join(", ")
            )
        }
        fts::Filter::Range {
            field,
            lower,
            upper,
        } => {
            let mut args = vec![py_repr(py, field)?];
            match lower {
                Limit::Included(v) => args.push(format!("ge={}", number_repr(v))),
                Limit::Excluded(v) => args.push(format!("gt={}", number_repr(v))),
                Limit::Unbounded => {}
            }
            match upper {
                Limit::Included(v) => args.push(format!("le={}", number_repr(v))),
                Limit::Excluded(v) => args.push(format!("lt={}", number_repr(v))),
                Limit::Unbounded => {}
            }
            format!("Filter.range({})", args.join(", "))
        }
        fts::Filter::And(all) => join(all, " & ")?,
        fts::Filter::Or(any) => join(any, " | ")?,
        fts::Filter::Not(inner) => format!("~{}", filter_repr(py, inner)?),
    })
}

/// The Snowball project's stop words for `language`.
#[pyfunction]
fn stop_words(language: &str) -> PyResult<Vec<String>> {
    Ok(snowball(language)?.words().to_vec())
}

pub(crate) fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = parent.py();
    let m = PyModule::new(py, "fts")?;
    m.add_class::<Index>()?;
    m.add_class::<Writer>()?;
    m.add_class::<Reader>()?;
    m.add_class::<Hit>()?;
    m.add_class::<Filter>()?;
    m.add_function(wrap_pyfunction!(stop_words, &m)?)?;
    let languages: Vec<&str> = fts::Language::ALL.iter().map(|l| l.name()).collect();
    m.add("LANGUAGES", PyTuple::new(py, languages)?)?;
    parent.add_submodule(&m)
}
