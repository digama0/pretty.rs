use std::borrow::Cow;
use std::cmp;
use std::fmt;
use std::io;
use std::ops::Deref;

#[cfg(feature = "termcolor")]
use termcolor::{ColorSpec, WriteColor};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Mode {
    Break,
    Flat,
}

/// The concrete document type. This type is not meant to be used directly. Instead use the static
/// functions on `Doc` or the methods on an `DocAllocator`.
///
/// The `B` parameter is used to abstract over pointers to `Doc`. See `RefDoc` and `BoxDoc` for how
/// it is used
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Doc<'a, B, A = ()> {
    Nil,
    Append(B, B),
    Group(B),
    Nest(usize, B),
    Space,
    Newline,
    Text(Cow<'a, str>),
    Annotated(A, B),
}

impl<'a, B, A, S> From<S> for Doc<'a, B, A>
where
    S: Into<Cow<'a, str>>,
{
    fn from(s: S) -> Doc<'a, B, A> {
        Doc::Text(s.into())
    }
}

/// Trait representing the operations necessary to render a document
pub trait Render {
    type Error;

    fn write_str(&mut self, s: &str) -> Result<usize, Self::Error>;

    fn write_str_all(&mut self, mut s: &str) -> Result<(), Self::Error> {
        while !s.is_empty() {
            let count = self.write_str(s)?;
            s = &s[count..];
        }
        Ok(())
    }
}

/// Writes to something implementing `std::io::Write`
pub struct IoWrite<W>(pub W);

impl<W> Render for IoWrite<W>
where
    W: io::Write,
{
    type Error = io::Error;

    fn write_str(&mut self, s: &str) -> io::Result<usize> {
        self.0.write(s.as_bytes())
    }

    fn write_str_all(&mut self, s: &str) -> io::Result<()> {
        self.0.write_all(s.as_bytes())
    }
}

/// Writes to something implementing `std::fmt::Write`
pub struct FmtWrite<W>(pub W);

impl<W> Render for FmtWrite<W>
where
    W: fmt::Write,
{
    type Error = fmt::Error;

    fn write_str(&mut self, s: &str) -> Result<usize, fmt::Error> {
        self.write_str_all(s).map(|_| s.len())
    }

    fn write_str_all(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s)
    }
}

/// Trait representing the operations necessary to write an annotated document.
pub trait RenderAnnotated<A>: Render {
    fn push_annotation(&mut self, annotation: &A) -> Result<(), Self::Error>;
    fn pop_annotation(&mut self) -> Result<(), Self::Error>;
}

impl<A, W> RenderAnnotated<A> for IoWrite<W>
where
    W: io::Write,
{
    fn push_annotation(&mut self, _: &A) -> Result<(), Self::Error> {
        Ok(())
    }

    fn pop_annotation(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<A, W> RenderAnnotated<A> for FmtWrite<W>
where
    W: fmt::Write,
{
    fn push_annotation(&mut self, _: &A) -> Result<(), Self::Error> {
        Ok(())
    }
    fn pop_annotation(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(feature = "termcolor")]
struct TermColored<W> {
    color_stack: Vec<ColorSpec>,
    writer: W,
}

#[cfg(feature = "termcolor")]
impl<W> Render for TermColored<W>
where
    W: io::Write,
{
    type Error = io::Error;

    fn write_str(&mut self, s: &str) -> io::Result<usize> {
        self.writer.write(s.as_bytes())
    }

    fn write_str_all(&mut self, s: &str) -> io::Result<()> {
        self.writer.write_all(s.as_bytes())
    }
}

#[cfg(feature = "termcolor")]
impl<W> RenderAnnotated<ColorSpec> for TermColored<W>
where
    W: WriteColor,
{
    fn push_annotation(&mut self, color: &ColorSpec) -> Result<(), Self::Error> {
        self.color_stack.push(color.clone());
        self.writer.set_color(color)
    }
    fn pop_annotation(&mut self) -> Result<(), Self::Error> {
        self.color_stack.pop();
        match self.color_stack.last() {
            Some(previous) => self.writer.set_color(previous),
            None => self.writer.reset(),
        }
    }
}

pub struct Pretty<'a, D, A>
where
    A: 'a,
    D: 'a,
{
    doc: &'a Doc<'a, D, A>,
    width: usize,
}

impl<'a, D, A> fmt::Display for Pretty<'a, D, A>
where
    D: Deref<Target = Doc<'a, D, A>>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.doc.render_fmt(self.width, f)
    }
}

impl<'a, B, A> Doc<'a, B, A> {
    /// Writes a rendered document to a `std::io::Write` object.
    #[inline]
    pub fn render<'b, W>(&'b self, width: usize, out: &mut W) -> io::Result<()>
    where
        B: Deref<Target = Doc<'b, B, A>>,
        W: ?Sized + io::Write,
    {
        self.render_raw(width, &mut IoWrite(out))
    }

    /// Writes a rendered document to a `std::fmt::Write` object.
    #[inline]
    pub fn render_fmt<'b, W>(&'b self, width: usize, out: &mut W) -> fmt::Result
    where
        B: Deref<Target = Doc<'b, B, A>>,
        W: ?Sized + fmt::Write,
    {
        self.render_raw(width, &mut FmtWrite(out))
    }

    /// Writes a rendered document to a `RenderAnnotated<A>` object.
    #[inline]
    pub fn render_raw<'b, W>(&'b self, width: usize, out: &mut W) -> Result<(), W::Error>
    where
        B: Deref<Target = Doc<'b, B, A>>,
        W: ?Sized + RenderAnnotated<A>,
    {
        best(self, width, out)
    }

    /// Returns a value which implements `std::fmt::Display`
    ///
    /// ```
    /// use pretty::Doc;
    /// let doc = Doc::<_>::group(
    ///     Doc::text("hello").append(Doc::space()).append(Doc::text("world"))
    /// );
    /// assert_eq!(format!("{}", doc.pretty(80)), "hello world");
    /// ```
    #[inline]
    pub fn pretty<'b>(&'b self, width: usize) -> Pretty<'b, B, A>
    where
        B: Deref<Target = Doc<'b, B, A>>,
    {
        Pretty { doc: self, width }
    }
}

#[cfg(feature = "termcolor")]
impl<'a, B> Doc<'a, B, ColorSpec> {
    #[inline]
    pub fn render_colored<'b, W>(&'b self, width: usize, out: W) -> io::Result<()>
    where
        B: Deref<Target = Doc<'b, B, ColorSpec>>,
        W: WriteColor,
    {
        best(
            self,
            width,
            &mut TermColored {
                color_stack: Vec::new(),
                writer: out,
            },
        )
    }
}

type Cmd<'a, B, A> = (usize, Mode, &'a Doc<'a, B, A>);

fn write_newline<W>(ind: usize, out: &mut W) -> Result<(), W::Error>
where
    W: ?Sized + Render,
{
    try!(out.write_str_all("\n"));
    write_spaces(ind, out)
}

fn write_spaces<W>(spaces: usize, out: &mut W) -> Result<(), W::Error>
where
    W: ?Sized + Render,
{
    macro_rules! make_spaces {
        () => {
            ""
        };
        ($s: tt $($t: tt)*) => {
            concat!("          ", make_spaces!($($t)*))
        };
    }
    const SPACES: &str = make_spaces!(,,,,,,,,,,);
    let mut inserted = 0;
    while inserted < spaces {
        let insert = cmp::min(SPACES.len(), spaces - inserted);
        inserted += try!(out.write_str(&SPACES[..insert]));
    }
    Ok(())
}

#[inline]
fn fitting<'a, B, A>(
    next: Cmd<'a, B, A>,
    bcmds: &[Cmd<'a, B, A>],
    fcmds: &mut Vec<Cmd<'a, B, A>>,
    mut rem: isize,
) -> bool
where
    B: Deref<Target = Doc<'a, B, A>>,
{
    let mut bidx = bcmds.len();
    fcmds.clear(); // clear from previous calls from best
    fcmds.push(next);
    while rem >= 0 {
        match fcmds.pop() {
            None => {
                if bidx == 0 {
                    // All commands have been processed
                    return true;
                } else {
                    fcmds.push(bcmds[bidx - 1]);
                    bidx -= 1;
                }
            }
            Some((ind, mode, doc)) => {
                match *doc {
                    Doc::Nil => {}
                    Doc::Append(ref ldoc, ref rdoc) => {
                        fcmds.push((ind, mode, rdoc));
                        // Since appended documents often appear in sequence on the left side we
                        // gain a slight performance increase by batching these pushes (avoiding
                        // to push and directly pop `Append` documents)
                        let mut doc = ldoc;
                        while let Doc::Append(ref l, ref r) = **doc {
                            fcmds.push((ind, mode, r));
                            doc = l;
                        }
                        fcmds.push((ind, mode, doc));
                    }
                    Doc::Group(ref doc) => {
                        fcmds.push((ind, mode, doc));
                    }
                    Doc::Nest(off, ref doc) => {
                        fcmds.push((ind + off, mode, doc));
                    }
                    Doc::Space => match mode {
                        Mode::Flat => {
                            rem -= 1;
                        }
                        Mode::Break => {
                            return true;
                        }
                    },
                    Doc::Newline => return true,
                    Doc::Text(ref str) => {
                        rem -= str.len() as isize;
                    }
                    Doc::Annotated(_, ref doc) => fcmds.push((ind, mode, doc)),
                }
            }
        }
    }
    false
}

#[inline]
fn best<'a, W, B, A>(doc: &'a Doc<'a, B, A>, width: usize, out: &mut W) -> Result<(), W::Error>
where
    B: Deref<Target = Doc<'a, B, A>>,
    W: ?Sized + RenderAnnotated<A>,
{
    let mut pos = 0usize;
    let mut bcmds = vec![(0usize, Mode::Break, doc)];
    let mut fcmds = vec![];
    let mut annotation_levels = vec![];

    while let Some((ind, mode, doc)) = bcmds.pop() {
        match *doc {
            Doc::Nil => {}
            Doc::Append(ref ldoc, ref rdoc) => {
                bcmds.push((ind, mode, rdoc));
                let mut doc = ldoc;
                while let Doc::Append(ref l, ref r) = **doc {
                    bcmds.push((ind, mode, r));
                    doc = l;
                }
                bcmds.push((ind, mode, doc));
            }
            Doc::Group(ref doc) => match mode {
                Mode::Flat => {
                    bcmds.push((ind, Mode::Flat, doc));
                }
                Mode::Break => {
                    let next = (ind, Mode::Flat, &**doc);
                    let rem = width as isize - pos as isize;
                    if fitting(next, &bcmds, &mut fcmds, rem) {
                        bcmds.push(next);
                    } else {
                        bcmds.push((ind, Mode::Break, doc));
                    }
                }
            },
            Doc::Nest(off, ref doc) => {
                bcmds.push((ind + off, mode, doc));
            }
            Doc::Space => match mode {
                Mode::Flat => {
                    try!(write_spaces(1, out));
                }
                Mode::Break => {
                    try!(write_newline(ind, out));
                    pos = ind;
                }
            },
            Doc::Newline => {
                try!(write_newline(ind, out));
                pos = ind;

                // Since this newline caused an early break we don't know if the remaining
                // documents fit the next line so recalculate if they fit
                fcmds.clear();
                let docs = bcmds.len()
                    - bcmds
                        .iter()
                        .rev()
                        .position(|t| t.1 == Mode::Break)
                        .unwrap_or_else(|| bcmds.len());
                fcmds.extend_from_slice(&bcmds[docs..]);
                if let Some(next) = fcmds.pop() {
                    let rem = width as isize - pos as isize;
                    if !fitting(next, &bcmds, &mut fcmds, rem) {
                        for &mut (_, ref mut mode, _) in &mut bcmds[docs..] {
                            *mode = Mode::Break;
                        }
                    }
                }
            }
            Doc::Text(ref s) => {
                try!(out.write_str_all(s));
                pos += s.len();
            }
            Doc::Annotated(ref ann, ref doc) => {
                try!(out.push_annotation(ann));
                annotation_levels.push(bcmds.len());
                bcmds.push((ind, mode, doc));
            }
        }

        if annotation_levels.last() == Some(&bcmds.len()) {
            annotation_levels.pop();
            try!(out.pop_annotation());
        }
    }
    Ok(())
}
