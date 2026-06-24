//! Type 2 charstring interpreter (Adobe Technical Note #5177).
//!
//! A Type 2 charstring is a byte program that builds one glyph outline by
//! pushing numeric operands and invoking path/hint/subroutine operators
//! against an argument stack. This module decodes that program into a
//! [`TtOutline`] of cubic-Bezier contours.
//!
//! ## Outline model
//!
//! Type 2 charstrings produce **cubic** Beziers (six-argument curves),
//! while the crate's [`TtOutline`] models **quadratic** TrueType contours
//! (on-/off-curve points). To keep one outline type across the crate, we
//! flatten each cubic into a short polyline of on-curve points. The
//! flattening tolerance is fixed in font units; downstream rasterizers
//! consume the resulting on-curve polygon directly. Straight-line
//! segments (`*lineto`) are emitted as single on-curve points.
//!
//! ## Width prefix (TN #5177 §3.1)
//!
//! The first stack-clearing operator may carry an extra leading operand:
//! the glyph width, encoded as a delta from `nominalWidthX`. We detect it
//! by the documented arity rule per operator and record it via
//! [`Interp::width`].
//!
//! ## Hints
//!
//! `hstem` / `vstem` / `hstemhm` / `vstemhm` declare stem hints; we only
//! need their *count* so that `hintmask` / `cntrmask` can consume the
//! correct number of trailing mask bytes. Hint geometry is not used for
//! outline reconstruction, so the stem values are counted and discarded.

use super::{subr_bias, Index};
use crate::outline::{Contour, Point, TtOutline};
use crate::parser::read_u8;

/// Maximum nesting depth for `callsubr` / `callgsubr` (TN #5177 Appendix
/// B caps this at 10).
const MAX_SUBR_DEPTH: usize = 10;

/// Cubic-flattening step count. A small fixed subdivision keeps the
/// on-curve polyline faithful at text sizes without unbounded growth.
const CUBIC_STEPS: u32 = 8;

/// Interpreter error (kept private; callers only see `Option`).
#[derive(Debug)]
pub struct CharstringError;

/// Type 2 charstring interpreter producing a flattened outline.
#[derive(Debug)]
pub struct Interp<'a> {
    global_subrs: Index<'a>,
    local_subrs: Index<'a>,
    nominal_width: f32,

    stack: Vec<f64>,
    /// Number of stem hints declared so far (drives hintmask byte count).
    n_stems: usize,
    /// Whether we've seen the optional width operand yet.
    width: Option<f32>,
    width_parsed: bool,

    /// Current pen position in font units.
    x: f64,
    y: f64,
    /// The contour under construction (on-curve points only).
    cur: Vec<Point>,
    contours: Vec<Contour>,
    open: bool,

    /// Transient array for `put`/`get` (TN #5177 §4.5). Sized lazily.
    transient: Vec<f64>,

    /// CFF2 mode (OpenType CFF2 §): charstrings carry no width prefix and
    /// no `endchar`; the `blend` (16) and `vsindex` (15) variation
    /// operators are recognised. In CFF (non-CFF2) mode they are absent.
    cff2: bool,
    /// Per-`vsindex` region scalars for the current variation instance.
    /// `vs_region_scalars[i]` holds the `k` scalars (one per region) the
    /// CFF2 `blend` operator multiplies its deltas by when `vsindex == i`.
    /// For the **default instance** every scalar is zero, so `blend`
    /// keeps only the default values; at a non-default instance the
    /// blended value is `default + Σ scalar_r · delta_r`.
    vs_region_scalars: Vec<Vec<f32>>,
    /// Active scalars `k` for the current `vsindex` (default index 0).
    active_scalars: Vec<f32>,
}

impl<'a> Interp<'a> {
    /// Create an interpreter bound to the global + local subr INDEXes and
    /// the per-font/FD `nominalWidthX`.
    pub fn new(global_subrs: Index<'a>, local_subrs: Index<'a>, nominal_width: f32) -> Self {
        Self {
            global_subrs,
            local_subrs,
            nominal_width,
            stack: Vec::with_capacity(48),
            n_stems: 0,
            width: None,
            width_parsed: false,
            x: 0.0,
            y: 0.0,
            cur: Vec::new(),
            contours: Vec::new(),
            open: false,
            transient: Vec::new(),
            cff2: false,
            vs_region_scalars: Vec::new(),
            active_scalars: Vec::new(),
        }
    }

    /// Create a CFF2 interpreter. `vsindex` 0 is active initially;
    /// `vs_region_scalars[i]` holds the region scalars for `vsindex == i`
    /// at the target variation instance (from the font's VariationStore).
    /// For the **default instance** pass all-zero scalar vectors (or
    /// vectors of the correct length filled with zeros) and `blend`
    /// collapses to the default values. The width prefix is suppressed
    /// (CFF2 charstrings carry no width).
    pub fn new_cff2(
        global_subrs: Index<'a>,
        local_subrs: Index<'a>,
        vs_region_scalars: Vec<Vec<f32>>,
    ) -> Self {
        let active_scalars = vs_region_scalars.first().cloned().unwrap_or_default();
        Self {
            global_subrs,
            local_subrs,
            nominal_width: 0.0,
            stack: Vec::with_capacity(48),
            n_stems: 0,
            width: None,
            // CFF2 has no width prefix: mark it already consumed so the
            // moveto/stem handlers never strip a "width" operand.
            width_parsed: true,
            x: 0.0,
            y: 0.0,
            cur: Vec::new(),
            contours: Vec::new(),
            open: false,
            transient: Vec::new(),
            cff2: true,
            vs_region_scalars,
            active_scalars,
        }
    }

    /// The glyph width recovered from the charstring, if one was encoded.
    pub fn width(&self) -> Option<f32> {
        self.width
    }

    /// Consume the interpreter, returning the assembled outline.
    pub fn into_outline(mut self) -> TtOutline {
        self.close_contour();
        let bounds = crate::outline::derive_bbox(&self.contours);
        TtOutline {
            contours: self.contours,
            bounds,
        }
    }

    /// Run the top-level charstring.
    pub fn run(&mut self, cs: &'a [u8]) -> Result<(), CharstringError> {
        self.exec(cs, 0)?;
        Ok(())
    }

    /// Execute one charstring (or subroutine) at nesting `depth`. Returns
    /// `Ok(true)` when an `endchar` terminated the whole glyph.
    fn exec(&mut self, cs: &'a [u8], depth: usize) -> Result<bool, CharstringError> {
        if depth > MAX_SUBR_DEPTH {
            return Err(CharstringError);
        }
        let mut i = 0;
        while i < cs.len() {
            let b0 = cs[i];
            match b0 {
                // --- numeric operands -----------------------------------
                28 => {
                    let hi = *cs.get(i + 1).ok_or(CharstringError)?;
                    let lo = *cs.get(i + 2).ok_or(CharstringError)?;
                    self.stack.push(i16::from_be_bytes([hi, lo]) as f64);
                    i += 3;
                }
                32..=246 => {
                    self.stack.push(b0 as f64 - 139.0);
                    i += 1;
                }
                247..=250 => {
                    let w = *cs.get(i + 1).ok_or(CharstringError)?;
                    self.stack
                        .push((b0 as f64 - 247.0) * 256.0 + w as f64 + 108.0);
                    i += 2;
                }
                251..=254 => {
                    let w = *cs.get(i + 1).ok_or(CharstringError)?;
                    self.stack
                        .push(-(b0 as f64 - 251.0) * 256.0 - w as f64 - 108.0);
                    i += 2;
                }
                255 => {
                    // 16.16 fixed.
                    let s = cs.get(i + 1..i + 5).ok_or(CharstringError)?;
                    let v = i32::from_be_bytes([s[0], s[1], s[2], s[3]]);
                    self.stack.push(v as f64 / 65536.0);
                    i += 5;
                }
                // --- operators ------------------------------------------
                _ => {
                    let mut consumed = 1;
                    let done = self.operator(b0, cs, i, &mut consumed, depth)?;
                    i += consumed;
                    if done {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// Dispatch a charstring operator. `i` is the operator's byte index;
    /// `*consumed` starts at 1 and the operator bumps it for any inline
    /// data bytes (e.g. hintmask mask bytes, the 12-escape second byte).
    fn operator(
        &mut self,
        b0: u8,
        cs: &'a [u8],
        i: usize,
        consumed: &mut usize,
        depth: usize,
    ) -> Result<bool, CharstringError> {
        match b0 {
            // CFF2 vsindex (15): select the active variation-region set.
            15 if self.cff2 => {
                if let Some(idx) = self.stack.pop() {
                    let i = idx.max(0.0) as usize;
                    self.active_scalars =
                        self.vs_region_scalars.get(i).cloned().unwrap_or_default();
                }
                self.stack.clear();
            }
            // CFF2 blend (16): interpolate blended operands at the active
            // variation instance. Stack: [... n default values, n*k
            // deltas, n], where the n*k deltas are grouped as n runs of k
            // (one delta per region). Each default value becomes
            // `default + Σ scalar_r · delta_r`; the deltas are consumed,
            // leaving the n blended values in place. At the default
            // instance every scalar is zero, so the defaults pass through.
            16 if self.cff2 => {
                if let Some(n_f) = self.stack.pop() {
                    let n = n_f.max(0.0) as usize;
                    let k = self.active_scalars.len();
                    let drop = n * k;
                    let len = self.stack.len();
                    if drop <= len && n <= len - drop {
                        let defaults_start = len - drop - n;
                        // Apply each region's delta to its default value.
                        for j in 0..n {
                            let mut acc = self.stack[defaults_start + j];
                            for (r, &scalar) in self.active_scalars.iter().enumerate() {
                                if scalar != 0.0 {
                                    let delta = self.stack[defaults_start + n + j * k + r];
                                    acc += scalar as f64 * delta;
                                }
                            }
                            self.stack[defaults_start + j] = acc;
                        }
                        // Drop the deltas; the blended defaults remain.
                        self.stack.truncate(defaults_start + n);
                    }
                }
            }
            // hstem / vstem / hstemhm / vstemhm
            1 | 3 | 18 | 23 => {
                self.count_stems();
                self.stack.clear();
            }
            // hintmask / cntrmask
            19 | 20 => {
                // An implicit vstem can precede the first mask (TN #5177
                // §4.3): any leftover operands are stem args.
                self.count_stems();
                self.stack.clear();
                let n_bytes = self.n_stems.div_ceil(8);
                // Skip the mask data bytes.
                let mask_start = i + 1;
                cs.get(mask_start..mask_start + n_bytes)
                    .ok_or(CharstringError)?;
                *consumed += n_bytes;
            }
            // rmoveto
            21 => {
                self.maybe_take_width(2);
                let n = self.stack.len();
                if n >= 2 {
                    let dx = self.stack[n - 2];
                    let dy = self.stack[n - 1];
                    self.moveto(dx, dy);
                }
                self.stack.clear();
            }
            // hmoveto
            22 => {
                self.maybe_take_width(1);
                if let Some(&dx) = self.stack.last() {
                    self.moveto(dx, 0.0);
                }
                self.stack.clear();
            }
            // vmoveto
            4 => {
                self.maybe_take_width(1);
                if let Some(&dy) = self.stack.last() {
                    self.moveto(0.0, dy);
                }
                self.stack.clear();
            }
            // rlineto
            5 => {
                let args = std::mem::take(&mut self.stack);
                let mut k = 0;
                while k + 1 < args.len() {
                    self.lineto(args[k], args[k + 1]);
                    k += 2;
                }
            }
            // hlineto
            6 => {
                let args = std::mem::take(&mut self.stack);
                let mut horizontal = true;
                for &a in &args {
                    if horizontal {
                        self.lineto(a, 0.0);
                    } else {
                        self.lineto(0.0, a);
                    }
                    horizontal = !horizontal;
                }
            }
            // vlineto
            7 => {
                let args = std::mem::take(&mut self.stack);
                let mut horizontal = false;
                for &a in &args {
                    if horizontal {
                        self.lineto(a, 0.0);
                    } else {
                        self.lineto(0.0, a);
                    }
                    horizontal = !horizontal;
                }
            }
            // rrcurveto
            8 => {
                let args = std::mem::take(&mut self.stack);
                let mut k = 0;
                while k + 5 < args.len() {
                    self.curveto(
                        args[k],
                        args[k + 1],
                        args[k + 2],
                        args[k + 3],
                        args[k + 4],
                        args[k + 5],
                    );
                    k += 6;
                }
            }
            // hhcurveto
            27 => {
                let args = std::mem::take(&mut self.stack);
                self.hhcurveto(&args);
            }
            // vvcurveto
            26 => {
                let args = std::mem::take(&mut self.stack);
                self.vvcurveto(&args);
            }
            // hvcurveto
            31 => {
                let args = std::mem::take(&mut self.stack);
                self.hv_vh_curveto(&args, true);
            }
            // vhcurveto
            30 => {
                let args = std::mem::take(&mut self.stack);
                self.hv_vh_curveto(&args, false);
            }
            // rcurveline
            24 => {
                let args = std::mem::take(&mut self.stack);
                let n = args.len();
                if n >= 6 {
                    let n_curves = (n - 2) / 6;
                    let mut k = 0;
                    for _ in 0..n_curves {
                        self.curveto(
                            args[k],
                            args[k + 1],
                            args[k + 2],
                            args[k + 3],
                            args[k + 4],
                            args[k + 5],
                        );
                        k += 6;
                    }
                    if k + 1 < n {
                        self.lineto(args[k], args[k + 1]);
                    }
                }
            }
            // rlinecurve
            25 => {
                let args = std::mem::take(&mut self.stack);
                let n = args.len();
                if n >= 6 {
                    let n_lines = (n - 6) / 2;
                    let mut k = 0;
                    for _ in 0..n_lines {
                        self.lineto(args[k], args[k + 1]);
                        k += 2;
                    }
                    if k + 5 < n {
                        self.curveto(
                            args[k],
                            args[k + 1],
                            args[k + 2],
                            args[k + 3],
                            args[k + 4],
                            args[k + 5],
                        );
                    }
                }
            }
            // callsubr
            10 => {
                let idx = self.stack.pop().ok_or(CharstringError)? as i32;
                let bias = subr_bias(self.local_subrs.count());
                let real = (idx + bias) as usize;
                let sub = self.local_subrs.get(real).ok_or(CharstringError)?;
                if self.exec(sub, depth + 1)? {
                    return Ok(true);
                }
            }
            // callgsubr
            29 => {
                let idx = self.stack.pop().ok_or(CharstringError)? as i32;
                let bias = subr_bias(self.global_subrs.count());
                let real = (idx + bias) as usize;
                let sub = self.global_subrs.get(real).ok_or(CharstringError)?;
                if self.exec(sub, depth + 1)? {
                    return Ok(true);
                }
            }
            // return
            11 => {
                // Caller's loop resumes after the callsubr/callgsubr.
                return Ok(false);
            }
            // endchar
            14 => {
                self.maybe_take_width(0);
                self.close_contour();
                return Ok(true);
            }
            // escape: two-byte operator
            12 => {
                let b1 = read_u8(cs, i + 1).map_err(|_| CharstringError)?;
                *consumed += 1;
                self.escaped_operator(b1)?;
            }
            _ => {
                // Reserved / unsupported operator: clear and continue.
                self.stack.clear();
            }
        }
        Ok(false)
    }

    /// Two-byte (escape 12) operators: arithmetic, storage, conditional,
    /// and the flex family.
    fn escaped_operator(&mut self, b1: u8) -> Result<(), CharstringError> {
        match b1 {
            // flex (12 35): two curves, fd hint argument ignored for shape.
            35 => {
                let args = std::mem::take(&mut self.stack);
                if args.len() >= 13 {
                    self.curveto(args[0], args[1], args[2], args[3], args[4], args[5]);
                    self.curveto(args[6], args[7], args[8], args[9], args[10], args[11]);
                }
            }
            // hflex (12 34)
            34 => {
                let a = std::mem::take(&mut self.stack);
                if a.len() >= 7 {
                    // dx1 dx2 dy2 dx3 dx4 dx5 dx6 ; dy held at base y.
                    self.curveto(a[0], 0.0, a[1], a[2], a[3], 0.0);
                    self.curveto(a[4], 0.0, a[5], -a[2], a[6], 0.0);
                }
            }
            // hflex1 (12 36)
            36 => {
                let a = std::mem::take(&mut self.stack);
                if a.len() >= 9 {
                    // dx1 dy1 dx2 dy2 dx3 dx4 dx5 dy5 dx6
                    self.curveto(a[0], a[1], a[2], a[3], a[4], 0.0);
                    let dy = -(a[1] + a[3] + a[7]);
                    self.curveto(a[5], 0.0, a[6], a[7], a[8], dy);
                }
            }
            // flex1 (12 37)
            37 => {
                let a = std::mem::take(&mut self.stack);
                if a.len() >= 11 {
                    // dx1 dy1 dx2 dy2 dx3 dy3 dx4 dy4 dx5 dy5 d6
                    let dx = a[0] + a[2] + a[4] + a[6] + a[8];
                    let dy = a[1] + a[3] + a[5] + a[7] + a[9];
                    self.curveto(a[0], a[1], a[2], a[3], a[4], a[5]);
                    if dx.abs() > dy.abs() {
                        self.curveto(a[6], a[7], a[8], a[9], a[10], -dy);
                    } else {
                        self.curveto(a[6], a[7], a[8], a[9], -dx, a[10]);
                    }
                }
            }
            // --- arithmetic / storage / conditional ---------------------
            // abs
            9 => self.unary(|x| x.abs()),
            // add
            10 => self.binary(|a, b| a + b),
            // sub
            11 => self.binary(|a, b| a - b),
            // div
            12 => self.binary(|a, b| if b != 0.0 { a / b } else { 0.0 }),
            // neg
            14 => self.unary(|x| -x),
            // random (deterministic 0.5; randomness not needed for shape)
            23 => self.stack.push(0.5),
            // mul
            24 => self.binary(|a, b| a * b),
            // sqrt
            26 => self.unary(|x| x.max(0.0).sqrt()),
            // drop
            18 => {
                self.stack.pop();
            }
            // exch
            28 => {
                let n = self.stack.len();
                if n >= 2 {
                    self.stack.swap(n - 1, n - 2);
                }
            }
            // index
            29 => {
                if let Some(j) = self.stack.pop() {
                    let n = self.stack.len();
                    let val = if j < 0.0 {
                        self.stack.last().copied()
                    } else {
                        let j = j as usize;
                        if j < n {
                            Some(self.stack[n - 1 - j])
                        } else {
                            None
                        }
                    };
                    if let Some(v) = val {
                        self.stack.push(v);
                    }
                }
            }
            // roll
            30 => {
                if self.stack.len() >= 2 {
                    let j = self.stack.pop().unwrap() as i64;
                    let nn = self.stack.pop().unwrap() as i64;
                    if nn > 0 && (nn as usize) <= self.stack.len() {
                        let n = nn as usize;
                        let len = self.stack.len();
                        let slice = &mut self.stack[len - n..];
                        let shift = j.rem_euclid(n as i64) as usize;
                        slice.rotate_right(shift);
                    }
                }
            }
            // dup
            27 => {
                if let Some(&v) = self.stack.last() {
                    self.stack.push(v);
                }
            }
            // put
            20 => {
                if self.stack.len() >= 2 {
                    let idx = self.stack.pop().unwrap() as usize;
                    let val = self.stack.pop().unwrap();
                    if idx >= self.transient.len() {
                        self.transient.resize(idx + 1, 0.0);
                    }
                    self.transient[idx] = val;
                }
            }
            // get
            21 => {
                if let Some(idx) = self.stack.pop() {
                    let v = self.transient.get(idx as usize).copied().unwrap_or(0.0);
                    self.stack.push(v);
                }
            }
            // and
            3 => self.binary(|a, b| ((a != 0.0) && (b != 0.0)) as i32 as f64),
            // or
            4 => self.binary(|a, b| ((a != 0.0) || (b != 0.0)) as i32 as f64),
            // not
            5 => self.unary(|x| (x == 0.0) as i32 as f64),
            // eq
            15 => self.binary(|a, b| (a == b) as i32 as f64),
            // ifelse
            22 => {
                if self.stack.len() >= 4 {
                    let v2 = self.stack.pop().unwrap();
                    let v1 = self.stack.pop().unwrap();
                    let s2 = self.stack.pop().unwrap();
                    let s1 = self.stack.pop().unwrap();
                    self.stack.push(if v1 <= v2 { s1 } else { s2 });
                }
            }
            _ => {
                // Unsupported escaped operator: clear stack defensively.
                self.stack.clear();
            }
        }
        Ok(())
    }

    // --- argument-stack helpers --------------------------------------

    fn unary(&mut self, f: impl Fn(f64) -> f64) {
        if let Some(x) = self.stack.pop() {
            self.stack.push(f(x));
        }
    }

    fn binary(&mut self, f: impl Fn(f64, f64) -> f64) {
        if self.stack.len() >= 2 {
            let b = self.stack.pop().unwrap();
            let a = self.stack.pop().unwrap();
            self.stack.push(f(a, b));
        }
    }

    /// Count the stem hints implied by the current operand stack and add
    /// them to `n_stems`. Stems come in pairs; the optional width prefix
    /// (odd leading operand) is stripped first.
    fn count_stems(&mut self) {
        self.maybe_take_width_stem();
        self.n_stems += self.stack.len() / 2;
    }

    /// Width detection for the first stem operator: if the operand count
    /// is odd, the leading operand is the width.
    fn maybe_take_width_stem(&mut self) {
        if self.width_parsed {
            return;
        }
        self.width_parsed = true;
        if self.stack.len() % 2 == 1 {
            let w = self.stack.remove(0);
            self.width = Some(self.nominal_width + w as f32);
        }
    }

    /// Width detection for a moveto/endchar that expects exactly `expected`
    /// path operands: a leading extra operand is the width.
    fn maybe_take_width(&mut self, expected: usize) {
        if self.width_parsed {
            return;
        }
        self.width_parsed = true;
        if self.stack.len() > expected {
            let w = self.stack.remove(0);
            self.width = Some(self.nominal_width + w as f32);
        }
    }

    // --- path construction -------------------------------------------

    fn moveto(&mut self, dx: f64, dy: f64) {
        self.close_contour();
        self.x += dx;
        self.y += dy;
        self.cur.push(self.pt());
        self.open = true;
    }

    fn lineto(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        self.cur.push(self.pt());
    }

    /// Append a cubic curve relative to the pen, flattening to on-curve
    /// points.
    #[allow(clippy::too_many_arguments)]
    fn curveto(&mut self, dx1: f64, dy1: f64, dx2: f64, dy2: f64, dx3: f64, dy3: f64) {
        let x0 = self.x;
        let y0 = self.y;
        let x1 = x0 + dx1;
        let y1 = y0 + dy1;
        let x2 = x1 + dx2;
        let y2 = y1 + dy2;
        let x3 = x2 + dx3;
        let y3 = y2 + dy3;
        for s in 1..=CUBIC_STEPS {
            let t = s as f64 / CUBIC_STEPS as f64;
            let mt = 1.0 - t;
            let a = mt * mt * mt;
            let b = 3.0 * mt * mt * t;
            let c = 3.0 * mt * t * t;
            let d = t * t * t;
            let px = a * x0 + b * x1 + c * x2 + d * x3;
            let py = a * y0 + b * y1 + c * y2 + d * y3;
            self.cur.push(Point {
                x: clamp_i16(px),
                y: clamp_i16(py),
                on_curve: true,
            });
        }
        self.x = x3;
        self.y = y3;
    }

    fn pt(&self) -> Point {
        Point {
            x: clamp_i16(self.x),
            y: clamp_i16(self.y),
            on_curve: true,
        }
    }

    fn close_contour(&mut self) {
        if self.open && !self.cur.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.cur),
            });
        }
        self.cur.clear();
        self.open = false;
    }

    // --- curve operators with alternating tangents -------------------

    /// hhcurveto: `dy1? {dxa dxb dyb dxc}+` — curves start/end horizontal.
    fn hhcurveto(&mut self, args: &[f64]) {
        let mut k = 0;
        let mut first_dy = 0.0;
        if args.len() % 4 == 1 {
            first_dy = args[0];
            k = 1;
        }
        let mut first = true;
        while k + 3 < args.len() {
            let dy1 = if first { first_dy } else { 0.0 };
            self.curveto(args[k], dy1, args[k + 1], args[k + 2], args[k + 3], 0.0);
            first = false;
            k += 4;
        }
    }

    /// vvcurveto: `dx1? {dya dxb dyb dyc}+` — curves start/end vertical.
    fn vvcurveto(&mut self, args: &[f64]) {
        let mut k = 0;
        let mut first_dx = 0.0;
        if args.len() % 4 == 1 {
            first_dx = args[0];
            k = 1;
        }
        let mut first = true;
        while k + 3 < args.len() {
            let dx1 = if first { first_dx } else { 0.0 };
            self.curveto(dx1, args[k], args[k + 1], args[k + 2], 0.0, args[k + 3]);
            first = false;
            k += 4;
        }
    }

    /// hvcurveto (`start_h == true`) / vhcurveto. Tangents alternate
    /// horizontal/vertical; the final curve may carry an extra `dxf`/`dyf`.
    fn hv_vh_curveto(&mut self, args: &[f64], start_h: bool) {
        let n = args.len();
        let mut k = 0;
        let mut horizontal = start_h;
        // Each curve consumes 4 args; the last curve may add a 5th (the
        // optional final df).
        while k + 4 <= n {
            let remaining = n - k;
            let last = remaining < 8;
            let df = if last && remaining == 5 {
                args[k + 4]
            } else {
                0.0
            };
            if horizontal {
                // start horizontal, end vertical
                // dx1 dx2 dy2 dy3 (+ df on x of last point)
                self.curveto(args[k], 0.0, args[k + 1], args[k + 2], df, args[k + 3]);
            } else {
                // start vertical, end horizontal
                // dy1 dx2 dy2 dx3 (+ df on y of last point)
                self.curveto(0.0, args[k], args[k + 1], args[k + 2], args[k + 3], df);
            }
            horizontal = !horizontal;
            k += 4;
        }
    }
}

fn clamp_i16(v: f64) -> i16 {
    let r = v.round();
    if r < i16::MIN as f64 {
        i16::MIN
    } else if r > i16::MAX as f64 {
        i16::MAX
    } else {
        r as i16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tables::cff::Index;

    /// Encode a small signed integer as a one-byte Type 2 operand
    /// (range -107..=107: byte = v + 139).
    fn op1(v: i32) -> u8 {
        (v + 139) as u8
    }

    /// CFF2 blend at the default instance (all scalars 0) keeps the
    /// default values: `100 40 1 blend` leaves `100`.
    #[test]
    fn cff2_blend_default_instance() {
        let empty = Index::empty_pub();
        // vsindex 0 has one region; default-instance scalar is 0.0.
        let mut interp = Interp::new_cff2(empty, empty, vec![vec![0.0f32]]);
        // Charstring: 1 0 rmoveto, then 100 40 1 blend 0 rlineto.
        let cs = vec![
            op1(1),
            op1(0),
            21, // rmoveto (1,0)
            op1(100),
            op1(40),
            op1(1),
            16,     // blend -> leaves 100
            op1(0), // dy
            5,      // rlineto: dx=100 (blended) dy=0
        ];
        interp.run(&cs).unwrap();
        let out = interp.into_outline();
        // Start at (1,0); rlineto +100,0 -> (101,0).
        let pts = &out.contours[0].points;
        assert_eq!((pts[0].x, pts[0].y), (1, 0));
        assert_eq!((pts[1].x, pts[1].y), (101, 0));
    }

    /// CFF2 blend at a non-default instance: scalar 0.5 on one region
    /// makes `100 40 1 blend` evaluate to 100 + 40*0.5 = 120.
    #[test]
    fn cff2_blend_scaled_instance() {
        let empty = Index::empty_pub();
        let mut interp = Interp::new_cff2(empty, empty, vec![vec![0.5f32]]);
        let cs = vec![
            op1(1),
            op1(0),
            21, // rmoveto (1,0)
            op1(100),
            op1(40),
            op1(1),
            16,     // blend -> 100 + 40*0.5 = 120
            op1(0), // dy
            5,      // rlineto
        ];
        interp.run(&cs).unwrap();
        let out = interp.into_outline();
        let pts = &out.contours[0].points;
        assert_eq!((pts[0].x, pts[0].y), (1, 0));
        // 1 + 120 = 121.
        assert_eq!((pts[1].x, pts[1].y), (121, 0));
    }
}
