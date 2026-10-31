//! ELF details `object` leaves to us: program headers, the dynamic section, mitigations, and
//! core-file notes (registers, signal, mapped files) for crash inspection.

use crate::binary::{Arch, Binary};
use crate::bytes::Reader;
use crate::{FormatError, hex_addr};
use object::Object;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

const MAX_PHDRS: usize = 4096;
const MAX_DYN: usize = 8192;
const MAX_NOTES: usize = 100_000;

/// Endian/class-aware reads over an ELF image.
#[derive(Clone, Copy)]
struct E<'a> {
    r: Reader<'a>,
    is64: bool,
    le: bool,
}

impl<'a> E<'a> {
    fn new(data: &'a [u8]) -> Result<Self, FormatError> {
        if !data.starts_with(b"\x7fELF") {
            return Err(FormatError::Malformed("no ELF magic".into()));
        }
        let r = Reader::new(data);
        let class = r.u8(4).ok_or_else(|| FormatError::Truncated("ELF ident".into()))?;
        let enc = r.u8(5).ok_or_else(|| FormatError::Truncated("ELF ident".into()))?;
        if !matches!(class, 1 | 2) || !matches!(enc, 1 | 2) {
            return Err(FormatError::Malformed("bad ELF class or data encoding".into()));
        }
        Ok(E { r, is64: class == 2, le: enc == 1 })
    }
    fn u16(&self, off: usize) -> Option<u16> {
        if self.le { self.r.u16_le(off) } else { self.r.u16_be(off) }
    }
    fn u32(&self, off: usize) -> Option<u32> {
        if self.le { self.r.u32_le(off) } else { self.r.u32_be(off) }
    }
    fn u64(&self, off: usize) -> Option<u64> {
        if self.le { self.r.u64_le(off) } else { self.r.u64_be(off) }
    }
    fn word(&self, off: usize) -> Option<u64> {
        if self.is64 { self.u64(off) } else { self.u32(off).map(u64::from) }
    }
    fn word_size(&self) -> usize {
        if self.is64 { 8 } else { 4 }
    }
}

/// One program header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgramHeader {
    pub kind: String,
    pub flags: String,
    pub offset: u64,
    #[serde(with = "hex_addr")]
    pub vaddr: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
    #[serde(skip)]
    pub p_type: u32,
    #[serde(skip)]
    pub p_flags: u32,
}

fn phdr_name(t: u32) -> String {
    match t {
        0 => "NULL".into(),
        1 => "LOAD".into(),
        2 => "DYNAMIC".into(),
        3 => "INTERP".into(),
        4 => "NOTE".into(),
        5 => "SHLIB".into(),
        6 => "PHDR".into(),
        7 => "TLS".into(),
        0x6474_e550 => "GNU_EH_FRAME".into(),
        0x6474_e551 => "GNU_STACK".into(),
        0x6474_e552 => "GNU_RELRO".into(),
        0x6474_e553 => "GNU_PROPERTY".into(),
        0x7000_0001 => "ARM_EXIDX".into(),
        other => format!("{other:#x}"),
    }
}

fn perm_string(f: u32) -> String {
    format!("{}{}{}", if f & 4 != 0 { 'r' } else { '-' }, if f & 2 != 0 { 'w' } else { '-' }, if f & 1 != 0 { 'x' } else { '-' })
}

/// The program headers of an ELF file.
pub fn program_headers(data: &[u8]) -> Result<Vec<ProgramHeader>, FormatError> {
    let e = E::new(data)?;
    let (phoff, phentsize, phnum) = if e.is64 { (e.u64(32), e.u16(54), e.u16(56)) } else { (e.u32(28).map(u64::from), e.u16(42), e.u16(44)) };
    let phoff = usize::try_from(phoff.ok_or_else(|| FormatError::Truncated("ELF header".into()))?).map_err(|_| FormatError::Malformed("phoff".into()))?;
    let phentsize = usize::from(phentsize.unwrap_or(0));
    let phnum = usize::from(phnum.unwrap_or(0)).min(MAX_PHDRS);
    let min = if e.is64 { 56 } else { 32 };
    if phnum > 0 && phentsize < min {
        return Err(FormatError::Malformed("program header entry too small".into()));
    }
    let mut out = Vec::new();
    for i in 0..phnum {
        let Some(o) = i.checked_mul(phentsize).and_then(|x| x.checked_add(phoff)) else { break };
        let ph = if e.is64 {
            (e.u32(o), e.u32(o + 4), e.u64(o + 8), e.u64(o + 16), e.u64(o + 32), e.u64(o + 40), e.u64(o + 48))
        } else {
            (
                e.u32(o),
                e.u32(o + 24),
                e.u32(o + 4).map(u64::from),
                e.u32(o + 8).map(u64::from),
                e.u32(o + 16).map(u64::from),
                e.u32(o + 20).map(u64::from),
                e.u32(o + 28).map(u64::from),
            )
        };
        let (Some(t), Some(f), Some(off), Some(va), Some(fs), Some(ms), Some(al)) = ph else { break };
        out.push(ProgramHeader {
            kind: phdr_name(t),
            flags: perm_string(f),
            offset: off,
            vaddr: va,
            file_size: fs,
            mem_size: ms,
            align: al,
            p_type: t,
            p_flags: f,
        });
    }
    Ok(out)
}

fn dyn_name(tag: u64) -> String {
    match tag {
        0 => "NULL",
        1 => "NEEDED",
        2 => "PLTRELSZ",
        3 => "PLTGOT",
        4 => "HASH",
        5 => "STRTAB",
        6 => "SYMTAB",
        7 => "RELA",
        8 => "RELASZ",
        9 => "RELAENT",
        10 => "STRSZ",
        11 => "SYMENT",
        12 => "INIT",
        13 => "FINI",
        14 => "SONAME",
        15 => "RPATH",
        16 => "SYMBOLIC",
        17 => "REL",
        18 => "RELSZ",
        19 => "RELENT",
        20 => "PLTREL",
        21 => "DEBUG",
        22 => "TEXTREL",
        23 => "JMPREL",
        24 => "BIND_NOW",
        25 => "INIT_ARRAY",
        26 => "FINI_ARRAY",
        27 => "INIT_ARRAYSZ",
        28 => "FINI_ARRAYSZ",
        29 => "RUNPATH",
        30 => "FLAGS",
        0x6fff_fef5 => "GNU_HASH",
        0x6fff_fffb => "FLAGS_1",
        0x6fff_fff0 => "VERSYM",
        0x6fff_fffe => "VERNEED",
        0x6fff_ffff => "VERNEEDNUM",
        _ => return format!("{tag:#x}"),
    }
    .into()
}

/// Map a virtual address to a file offset using PT_LOAD headers.
fn va_to_off(phdrs: &[ProgramHeader], va: u64) -> Option<u64> {
    phdrs.iter().filter(|p| p.p_type == 1).find(|p| va >= p.vaddr && va - p.vaddr < p.file_size).map(|p| p.offset + (va - p.vaddr))
}

/// Add ELF facts to a parsed binary: image base, interpreter, needed libraries, RELRO, NX, PIE.
pub fn enrich(data: &[u8], file: &object::File<'_>, b: &mut Binary) {
    let Ok(e) = E::new(data) else { return };
    let phdrs = match program_headers(data) {
        Ok(p) => p,
        Err(err) => {
            b.warnings.push(format!("program headers: {err}"));
            Vec::new()
        }
    };
    let e_type = e.u16(16).unwrap_or(0);
    if let Some(base) = phdrs.iter().filter(|p| p.p_type == 1).map(|p| p.vaddr).min() {
        b.image_base = base;
    }
    // Dynamic section.
    let mut dyns: Vec<(u64, u64)> = Vec::new();
    if let Some(d) = phdrs.iter().find(|p| p.p_type == 2) {
        let ent = 2 * e.word_size();
        let n = usize::try_from(d.file_size).unwrap_or(0) / ent.max(1);
        let start = usize::try_from(d.offset).unwrap_or(usize::MAX);
        for i in 0..n.min(MAX_DYN) {
            let Some(o) = start.checked_add(i * ent) else { break };
            let (Some(tag), Some(val)) = (e.word(o), e.word(o + e.word_size())) else { break };
            if tag == 0 {
                break;
            }
            dyns.push((tag, val));
        }
    }
    let strtab = dyns.iter().find(|(t, _)| *t == 5).and_then(|(_, v)| va_to_off(&phdrs, *v));
    let dstr = |v: u64| -> Option<String> {
        let off = usize::try_from(strtab?.checked_add(v)?).ok()?;
        e.r.cstr(off, 4096)
    };
    let mut flags = 0u64;
    let mut flags1 = 0u64;
    let mut bind_now = false;
    let mut dyn_json = Vec::new();
    for (tag, val) in &dyns {
        match tag {
            1 => {
                if let Some(s) = dstr(*val) {
                    b.libraries.push(s);
                }
            }
            14 => {
                if let Some(s) = dstr(*val) {
                    b.info.insert("soname".into(), s);
                }
            }
            15 => {
                if let Some(s) = dstr(*val) {
                    b.info.insert("rpath".into(), s);
                }
            }
            29 => {
                if let Some(s) = dstr(*val) {
                    b.info.insert("runpath".into(), s);
                }
            }
            24 => bind_now = true,
            30 => flags = *val,
            0x6fff_fffb => flags1 = *val,
            _ => {}
        }
        if dyn_json.len() < 512 {
            dyn_json.push(json!({"tag": dyn_name(*tag), "value": format!("{val:#x}")}));
        }
    }
    bind_now |= flags & 0x8 != 0 || flags1 & 0x1 != 0;
    let interp = phdrs.iter().find(|p| p.p_type == 3).and_then(|p| e.r.cstr(usize::try_from(p.offset).ok()?, usize::try_from(p.file_size).ok()?.min(4096)));
    if let Some(i) = &interp {
        b.info.insert("interpreter".into(), i.clone());
    }
    // Mitigations.
    b.mitigations.nx = Some(phdrs.iter().find(|p| p.p_type == 0x6474_e551).is_some_and(|p| p.p_flags & 1 == 0));
    let relro = phdrs.iter().any(|p| p.p_type == 0x6474_e552);
    b.mitigations.relro = Some(
        match (relro, bind_now) {
            (false, _) => "none",
            (true, false) => "partial",
            (true, true) => "full",
        }
        .into(),
    );
    if e_type == 3 {
        let pie = interp.is_some() || flags1 & 0x0800_0000 != 0;
        b.mitigations.pie = Some(pie);
        if pie {
            b.kind = "executable";
        }
    } else if e_type == 2 {
        b.mitigations.pie = Some(false);
    }
    // Compiler identification from .comment.
    if let Some(sec) = b.sections.iter().find(|s| s.name == ".comment")
        && let Some(off) = sec.file_offset.and_then(|o| usize::try_from(o).ok())
    {
        let n = usize::try_from(sec.file_size).unwrap_or(0).min(4096);
        if let Some(bytes) = data.get(off..off.saturating_add(n)) {
            let parts: Vec<String> = bytes.split(|&c| c == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect();
            if !parts.is_empty() {
                b.info.insert("compiler".into(), parts.join("; "));
            }
        }
    }
    let _ = file.architecture();
    b.details = json!({
        "elf_type": match e_type { 1 => "REL", 2 => "EXEC", 3 => "DYN", 4 => "CORE", _ => "OTHER" },
        "program_headers": phdrs,
        "dynamic": dyn_json,
        "dt_flags": format!("{flags:#x}"),
        "dt_flags_1": format!("{flags1:#x}"),
    });
}

// ---------------------------------------------------------------------------------------------
// Dynamic symbols with their versions

/// Most dynamic symbols read from one file.
const MAX_DYNSYMS: usize = 400_000;
/// Most version definitions or needs read from one file.
const MAX_VERSIONS: usize = 65_536;

/// One symbol of the dynamic symbol table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DynSymbol {
    pub name: String,
    #[serde(with = "hex_addr")]
    pub addr: u64,
    pub size: u64,
    /// function, object, tls, ifunc, common, other
    pub kind: &'static str,
    /// global, weak, unique
    pub binding: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The library a needed version belongs to (`.gnu.version_r`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
}

/// The dynamic symbol table, split into what the file offers and what it needs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DynamicSymbols {
    pub exports: Vec<DynSymbol>,
    pub imports: Vec<DynSymbol>,
    /// A cap or the end of the file stopped the walk.
    pub truncated: bool,
    pub warnings: Vec<String>,
}

fn section_bytes<'a>(data: &'a [u8], b: &Binary, name: &str) -> Option<&'a [u8]> {
    let s = b.sections.iter().find(|s| s.name == name)?;
    let off = usize::try_from(s.file_offset?).ok()?;
    let len = usize::try_from(s.file_size).ok()?;
    data.get(off..off.checked_add(len)?)
}

fn str_at(strs: &[u8], off: u32) -> Option<String> {
    let rest = strs.get(off as usize..)?;
    // Only the capped part is searched: a table with no NUL would otherwise be read to its end for
    // every symbol.
    let rest = rest.get(..rest.len().min(4096))?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Some(String::from_utf8_lossy(rest.get(..end)?).into_owned())
}

/// Version definitions: index → name.
fn version_defs(e: &E<'_>, base: usize, strs: &[u8]) -> BTreeMap<u16, String> {
    let mut out = BTreeMap::new();
    let mut pos = 0usize;
    for _ in 0..MAX_VERSIONS {
        let Some(at) = base.checked_add(pos) else { break };
        let (Some(ndx), Some(aux), Some(next)) = (e.u16(at.saturating_add(4)), e.u32(at.saturating_add(12)), e.u32(at.saturating_add(16))) else { break };
        if let Some(name) = at.checked_add(aux as usize).and_then(|a| e.u32(a)).and_then(|n| str_at(strs, n)) {
            out.insert(ndx & 0x7fff, name);
        }
        if next == 0 {
            break;
        }
        let Some(p) = pos.checked_add(next as usize) else { break };
        pos = p;
    }
    out
}

/// Version needs: index → (name, library).
fn version_needs(e: &E<'_>, base: usize, strs: &[u8]) -> BTreeMap<u16, (String, String)> {
    let mut out = BTreeMap::new();
    let mut pos = 0usize;
    let mut budget = MAX_VERSIONS;
    while budget > 0 {
        budget -= 1;
        let Some(at) = base.checked_add(pos) else { break };
        let (Some(cnt), Some(file), Some(aux), Some(next)) =
            (e.u16(at.saturating_add(2)), e.u32(at.saturating_add(4)), e.u32(at.saturating_add(8)), e.u32(at.saturating_add(12)))
        else {
            break;
        };
        let library = str_at(strs, file).unwrap_or_default();
        let Some(mut a) = at.checked_add(aux as usize) else { break };
        for _ in 0..cnt {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let (Some(other), Some(name), Some(anext)) = (e.u16(a.saturating_add(6)), e.u32(a.saturating_add(8)), e.u32(a.saturating_add(12))) else { break };
            if let Some(n) = str_at(strs, name) {
                out.insert(other & 0x7fff, (n, library.clone()));
            }
            if anext == 0 {
                break;
            }
            let Some(na) = a.checked_add(anext as usize) else { break };
            a = na;
        }
        if next == 0 {
            break;
        }
        let Some(p) = pos.checked_add(next as usize) else { break };
        pos = p;
    }
    out
}

/// Read `.dynsym` with its version tables. `None` when the file isn't ELF or has no named
/// dynamic symbol section (a file with its section headers removed; use [`Binary`]'s lists then).
pub fn dynamic_symbols(data: &[u8], b: &Binary) -> Option<DynamicSymbols> {
    let e = E::new(data).ok()?;
    let syms = section_bytes(data, b, ".dynsym")?;
    let strs = section_bytes(data, b, ".dynstr")?;
    let off_of = |name: &str| b.sections.iter().find(|s| s.name == name).and_then(|s| s.file_offset).and_then(|o| usize::try_from(o).ok());
    let defs = off_of(".gnu.version_d").map(|o| version_defs(&e, o, strs)).unwrap_or_default();
    let needs = off_of(".gnu.version_r").map(|o| version_needs(&e, o, strs)).unwrap_or_default();
    let versym = off_of(".gnu.version");
    let syms_off = off_of(".dynsym")?;
    let ent = if e.is64 { 24usize } else { 16 };
    let count = syms.len() / ent;
    let mut out = DynamicSymbols::default();
    if count > MAX_DYNSYMS {
        out.truncated = true;
        out.warnings.push(format!("only the first {MAX_DYNSYMS} of {count} dynamic symbols were read"));
    }
    for i in 0..count.min(MAX_DYNSYMS) {
        let at = syms_off.checked_add(i.checked_mul(ent)?)?;
        let (name, info, other, shndx, value, size) = if e.is64 {
            (e.u32(at)?, e.r.u8(at + 4)?, e.r.u8(at + 5)?, e.u16(at + 6)?, e.u64(at + 8)?, e.u64(at + 16)?)
        } else {
            (e.u32(at)?, e.r.u8(at + 12)?, e.r.u8(at + 13)?, e.u16(at + 14)?, u64::from(e.u32(at + 4)?), u64::from(e.u32(at + 8)?))
        };
        let binding = match info >> 4 {
            1 => "global",
            2 => "weak",
            10 => "unique",
            _ => continue,
        };
        let kind = match info & 0xf {
            0 | 2 => "function",
            1 => "object",
            5 => "common",
            6 => "tls",
            10 => "ifunc",
            3 | 4 => continue,
            _ => "other",
        };
        let Some(text) = str_at(strs, name).filter(|t| !t.is_empty()) else { continue };
        let index = versym.and_then(|v| v.checked_add(i.checked_mul(2)?)).and_then(|v| e.u16(v)).map(|v| v & 0x7fff);
        let defined = shndx != 0;
        let (version, library) = match index {
            Some(ix) if ix >= 2 => {
                if defined {
                    (defs.get(&ix).cloned(), None)
                } else {
                    needs.get(&ix).cloned().map_or((None, None), |(v, l)| (Some(v), Some(l)))
                }
            }
            _ => (None, None),
        };
        let row = DynSymbol { name: text, addr: value, size, kind, binding, version, library };
        if defined {
            // Hidden and internal symbols aren't visible to other programs.
            if !matches!(other & 3, 1 | 2) {
                out.exports.push(row);
            }
        } else {
            out.imports.push(row);
        }
    }
    Some(out)
}

/// Name of an ELF dynamic relocation type.
pub fn reloc_name(arch: object::Architecture, t: u32) -> String {
    use object::Architecture as A;
    let n = match (arch, t) {
        (A::X86_64, 1) => "R_X86_64_64",
        (A::X86_64, 2) => "R_X86_64_PC32",
        (A::X86_64, 5) => "R_X86_64_COPY",
        (A::X86_64, 6) => "R_X86_64_GLOB_DAT",
        (A::X86_64, 7) => "R_X86_64_JUMP_SLOT",
        (A::X86_64, 8) => "R_X86_64_RELATIVE",
        (A::X86_64, 16) => "R_X86_64_DTPMOD64",
        (A::X86_64, 17) => "R_X86_64_DTPOFF64",
        (A::X86_64, 18) => "R_X86_64_TPOFF64",
        (A::X86_64, 37) => "R_X86_64_IRELATIVE",
        (A::Aarch64, 257) => "R_AARCH64_ABS64",
        (A::Aarch64, 1024) => "R_AARCH64_COPY",
        (A::Aarch64, 1025) => "R_AARCH64_GLOB_DAT",
        (A::Aarch64, 1026) => "R_AARCH64_JUMP_SLOT",
        (A::Aarch64, 1027) => "R_AARCH64_RELATIVE",
        (A::Aarch64, 1030) => "R_AARCH64_TLS_TPREL64",
        (A::Aarch64, 1031) => "R_AARCH64_TLSDESC",
        (A::Aarch64, 1032) => "R_AARCH64_IRELATIVE",
        (A::I386, 1) => "R_386_32",
        (A::I386, 2) => "R_386_PC32",
        (A::I386, 5) => "R_386_COPY",
        (A::I386, 6) => "R_386_GLOB_DAT",
        (A::I386, 7) => "R_386_JUMP_SLOT",
        (A::I386, 8) => "R_386_RELATIVE",
        (A::Arm, 2) => "R_ARM_ABS32",
        (A::Arm, 21) => "R_ARM_GLOB_DAT",
        (A::Arm, 22) => "R_ARM_JUMP_SLOT",
        (A::Arm, 23) => "R_ARM_RELATIVE",
        _ => return format!("type_{t}"),
    };
    n.into()
}

// ---------------------------------------------------------------------------------------------
// Core files

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Register {
    pub name: &'static str,
    #[serde(with = "hex_addr")]
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreThread {
    pub pid: u32,
    pub signal: u16,
    pub registers: Vec<Register>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreProcess {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SigInfo {
    pub signo: i32,
    pub name: &'static str,
    pub code: i32,
    pub errno: i32,
    #[serde(with = "hex_addr::opt")]
    pub fault_addr: Option<u64>,
    /// Plain-English meaning of the signal and code.
    pub meaning: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MappedFile {
    #[serde(with = "hex_addr")]
    pub start: u64,
    #[serde(with = "hex_addr")]
    pub end: u64,
    pub file_offset: u64,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NoteInfo {
    pub owner: String,
    pub kind: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoreDump {
    pub arch: Arch,
    pub bits: u8,
    pub threads: Vec<CoreThread>,
    pub process: Option<CoreProcess>,
    pub signal: Option<SigInfo>,
    pub mapped_files: Vec<MappedFile>,
    pub auxv: Vec<(String, String)>,
    pub notes: Vec<NoteInfo>,
    pub memory_segments: Vec<ProgramHeader>,
    pub warnings: Vec<String>,
}

impl CoreDump {
    /// The crashing thread's program counter.
    pub fn pc(&self) -> Option<u64> {
        let t = self.threads.first()?;
        t.registers.iter().find(|r| matches!(r.name, "rip" | "pc" | "eip")).map(|r| r.value)
    }

    /// The mapped file (and offset in it) holding `addr`.
    pub fn locate(&self, addr: u64) -> Option<(&MappedFile, u64)> {
        let m = self.mapped_files.iter().find(|m| addr >= m.start && addr < m.end)?;
        Some((m, m.file_offset.saturating_add(addr - m.start)))
    }
}

const X64_REGS: [&str; 27] = [
    "r15", "r14", "r13", "r12", "rbp", "rbx", "r11", "r10", "r9", "r8", "rax", "rcx", "rdx", "rsi", "rdi", "orig_rax", "rip", "cs", "eflags", "rsp", "ss",
    "fs_base", "gs_base", "ds", "es", "fs", "gs",
];
const I386_REGS: [&str; 17] = ["ebx", "ecx", "edx", "esi", "edi", "ebp", "eax", "ds", "es", "fs", "gs", "orig_eax", "eip", "cs", "eflags", "esp", "ss"];
const A64_REGS: [&str; 34] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20", "x21", "x22",
    "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30", "sp", "pc", "pstate",
];

pub fn signal_name(n: i32) -> &'static str {
    match n {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        31 => "SIGSYS",
        _ => "unknown",
    }
}

fn signal_meaning(signo: i32, code: i32) -> String {
    match (signo, code) {
        (11, 1) => "Read or wrote memory that isn't mapped (often a null or dangling pointer).".into(),
        (11, 2) => "Touched memory it isn't allowed to (wrong permissions, e.g. writing read-only data).".into(),
        (11, _) => "Invalid memory access (segmentation fault).".into(),
        (6, _) => "The program aborted itself (a failed assertion, a detected corruption, or abort()).".into(),
        (4, _) => "Ran an invalid instruction (corrupted code, a trap like ud2, or the wrong CPU).".into(),
        (8, 1) | (8, 2) => "Integer division by zero or overflow.".into(),
        (8, _) => "Arithmetic error.".into(),
        (7, _) => "Bus error: misaligned access or a mapped file that shrank.".into(),
        (5, _) => "Hit a breakpoint or trap.".into(),
        (31, _) => "Made a forbidden system call (sandbox or seccomp).".into(),
        _ => format!("Stopped by {}.", signal_name(signo)),
    }
}

fn auxv_name(t: u64) -> String {
    match t {
        3 => "AT_PHDR",
        4 => "AT_PHENT",
        5 => "AT_PHNUM",
        6 => "AT_PAGESZ",
        7 => "AT_BASE",
        9 => "AT_ENTRY",
        15 => "AT_PLATFORM",
        16 => "AT_HWCAP",
        25 => "AT_RANDOM",
        31 => "AT_EXECFN",
        33 => "AT_SYSINFO_EHDR",
        _ => return format!("AT_{t}"),
    }
    .into()
}

/// Parse an ELF core file's notes and memory segments.
pub fn parse_core(data: &[u8]) -> Result<CoreDump, FormatError> {
    let e = E::new(data)?;
    if e.u16(16) != Some(4) {
        return Err(FormatError::Unsupported("not an ELF core file (e_type is not CORE)".into()));
    }
    let machine = e.u16(18).unwrap_or(0);
    let arch = match machine {
        62 => Arch::X86_64,
        3 => Arch::X86,
        183 => Arch::Aarch64,
        40 => Arch::Arm,
        _ => Arch::Other,
    };
    let phdrs = program_headers(data)?;
    let mut core = CoreDump {
        arch,
        bits: if e.is64 { 64 } else { 32 },
        threads: Vec::new(),
        process: None,
        signal: None,
        mapped_files: Vec::new(),
        auxv: Vec::new(),
        notes: Vec::new(),
        memory_segments: phdrs.iter().filter(|p| p.p_type == 1).cloned().collect(),
        warnings: Vec::new(),
    };
    let ws = e.word_size();
    for ph in phdrs.iter().filter(|p| p.p_type == 4) {
        let start = usize::try_from(ph.offset).map_err(|_| FormatError::Malformed("note offset".into()))?;
        let len = usize::try_from(ph.file_size).map_err(|_| FormatError::Malformed("note size".into()))?;
        let end = start.saturating_add(len).min(data.len());
        if start.saturating_add(len) > data.len() {
            core.warnings.push("a note segment runs past the end of the file; read what is there".into());
        }
        let mut pos = start;
        while pos.saturating_add(12) <= end && core.notes.len() < MAX_NOTES {
            let (Some(namesz), Some(descsz), Some(ty)) = (e.u32(pos), e.u32(pos + 4), e.u32(pos + 8)) else { break };
            let namesz = namesz as usize;
            let descsz = descsz as usize;
            let name_off = pos + 12;
            let desc_off = name_off.saturating_add(namesz.saturating_add(3) & !3);
            let next = desc_off.saturating_add(descsz.saturating_add(3) & !3);
            if desc_off.saturating_add(descsz) > end {
                core.warnings.push("a note is cut short; stopped reading notes".into());
                break;
            }
            let owner = e.r.cstr(name_off, namesz).unwrap_or_default();
            let desc = data.get(desc_off..desc_off + descsz).unwrap_or(&[]);
            let d = E { r: Reader::new(desc), ..e };
            let kind = match ty {
                1 => "NT_PRSTATUS".to_string(),
                2 => "NT_PRFPREG".into(),
                3 => "NT_PRPSINFO".into(),
                6 => "NT_AUXV".into(),
                0x5349_4749 => "NT_SIGINFO".into(),
                0x4649_4c45 => "NT_FILE".into(),
                0x202 => "NT_X86_XSTATE".into(),
                t => format!("{t:#x}"),
            };
            core.notes.push(NoteInfo { owner: owner.clone(), kind, size: descsz as u64 });
            if owner == "CORE" {
                match ty {
                    1 => {
                        let (pid_off, reg_off, names): (usize, usize, &[&'static str]) = match (arch, e.is64) {
                            (Arch::X86_64, true) => (32, 112, &X64_REGS),
                            (Arch::Aarch64, true) => (32, 112, &A64_REGS),
                            (Arch::X86, false) => (24, 72, &I386_REGS),
                            (_, true) => (32, 112, &[]),
                            (_, false) => (24, 72, &[]),
                        };
                        let registers =
                            names.iter().enumerate().filter_map(|(i, n)| d.word(reg_off + i * ws).map(|v| Register { name: n, value: v })).collect();
                        core.threads.push(CoreThread { pid: d.u32(pid_off).unwrap_or(0), signal: d.u16(12).unwrap_or(0), registers });
                    }
                    3 => {
                        let (pid, ppid, fname, args) = if e.is64 { (24, 28, 40, 56) } else { (12, 16, 28, 44) };
                        core.process = Some(CoreProcess {
                            pid: d.u32(pid).unwrap_or(0),
                            ppid: d.u32(ppid).unwrap_or(0),
                            name: d.r.cstr(fname, 16).unwrap_or_default(),
                            args: d.r.cstr(args, 80).unwrap_or_default().trim_end().to_string(),
                        });
                    }
                    0x5349_4749 => {
                        let signo = d.u32(0).unwrap_or(0) as i32;
                        let errno = d.u32(4).unwrap_or(0) as i32;
                        let code = d.u32(8).unwrap_or(0) as i32;
                        let fault = if matches!(signo, 4 | 7 | 8 | 11) { d.word(if e.is64 { 16 } else { 12 }) } else { None };
                        core.signal = Some(SigInfo { signo, name: signal_name(signo), code, errno, fault_addr: fault, meaning: signal_meaning(signo, code) });
                    }
                    0x4649_4c45 => {
                        let count = usize::try_from(d.word(0).unwrap_or(0)).unwrap_or(0).min(65_536);
                        let page = d.word(ws).unwrap_or(4096);
                        let table = 2 * ws;
                        let mut names_off = table.saturating_add(count.saturating_mul(3 * ws));
                        for i in 0..count {
                            let o = table + i * 3 * ws;
                            let (Some(s), Some(en), Some(po)) = (d.word(o), d.word(o + ws), d.word(o + 2 * ws)) else { break };
                            let path = d.r.cstr(names_off, 4096).unwrap_or_default();
                            names_off = names_off.saturating_add(path.len() + 1);
                            core.mapped_files.push(MappedFile { start: s, end: en, file_offset: po.saturating_mul(page), path });
                        }
                    }
                    6 => {
                        let mut o = 0;
                        while let (Some(t), Some(v)) = (d.word(o), d.word(o + ws)) {
                            if t == 0 || core.auxv.len() >= 256 {
                                break;
                            }
                            let value = if t == 31 || t == 15 { format!("{v:#x} (pointer to a string in the process)") } else { format!("{v:#x}") };
                            core.auxv.push((auxv_name(t), value));
                            o += 2 * ws;
                        }
                    }
                    _ => {}
                }
            }
            if next <= pos {
                break;
            }
            pos = next;
        }
    }
    if core.threads.is_empty() {
        core.warnings.push("no thread status (NT_PRSTATUS) note; registers are unknown".into());
    }
    if core.signal.is_none()
        && let Some(t) = core.threads.first()
        && t.signal != 0
    {
        let signo = i32::from(t.signal);
        core.signal = Some(SigInfo { signo, name: signal_name(signo), code: 0, errno: 0, fault_addr: None, meaning: signal_meaning(signo, 0) });
    }
    Ok(core)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_string_stops_at_its_nul_or_the_cap() {
        assert_eq!(super::str_at(b"ab\0cd\0", 3).as_deref(), Some("cd"));
        assert_eq!(super::str_at(b"ab\0", 9), None);
        assert_eq!(super::str_at(&vec![b'a'; 10_000], 0).map(|s| s.len()), Some(4096), "no NUL anywhere");
        let mut far = vec![b'a'; 5000];
        far.push(0);
        assert_eq!(super::str_at(&far, 0).map(|s| s.len()), Some(4096), "a NUL past the cap");
    }

    use super::*;

    #[test]
    fn core_notes_round_trip() {
        let core = craft_anything_testkit::elf64_core(0x401234, 0xdead);
        let c = parse_core(&core).unwrap();
        assert_eq!(c.arch, Arch::X86_64);
        assert_eq!(c.pc(), Some(0x401234));
        let s = c.signal.as_ref().unwrap();
        assert_eq!(s.name, "SIGSEGV");
        assert_eq!(s.fault_addr, Some(0xdead));
        assert_eq!(c.process.as_ref().unwrap().name, "crashy");
        assert_eq!(c.mapped_files.len(), 2);
        assert_eq!(c.mapped_files[1].path, "/usr/lib/libc.so.6");
        let (m, off) = c.locate(0x400010).unwrap();
        assert_eq!(m.path, "/usr/bin/crashy");
        assert_eq!(off, 0x10);
    }

    #[test]
    fn damaged_cores_never_panic() {
        let core = craft_anything_testkit::elf64_core(0x401234, 0xdead);
        for m in craft_anything_testkit::mutations(&core) {
            let _ = parse_core(&m);
        }
    }

    #[test]
    fn program_headers_and_mitigations() {
        let (elf, _) = craft_anything_testkit::elf64(craft_anything_testkit::ElfArch::X86_64, true);
        let ph = program_headers(&elf).unwrap();
        assert!(ph.iter().any(|p| p.kind == "GNU_STACK"));
        let b = crate::parse_binary(&elf).unwrap();
        assert_eq!(b.mitigations.nx, Some(true));
        assert_eq!(b.mitigations.relro.as_deref(), Some("none"));
        assert_eq!(b.libraries, vec!["libc.so.6".to_string()]);
        assert_eq!(b.info.get("interpreter").map(String::as_str), Some("/lib64/ld-linux-x86-64.so.2"));
    }
}
