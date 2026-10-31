//! Bounded-memory, ICC-tagged PDF pages for photographic prints/contact sheets.
use crate::{ExportColorSpace, ExportImage};
use serde::{Deserialize, Serialize};
use std::{
    io::{Seek, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrintLayout {
    pub width_mm: f32,
    pub height_mm: f32,
    pub margin_mm: f32,
    pub gap_mm: f32,
    pub columns: u32,
    pub rows: u32,
    pub ppi: u32,
    pub fill: bool,
    pub captions: bool,
}
impl Default for PrintLayout {
    fn default() -> Self {
        Self {
            width_mm: 210.0,
            height_mm: 297.0,
            margin_mm: 10.0,
            gap_mm: 5.0,
            columns: 1,
            rows: 1,
            ppi: 300,
            fill: false,
            captions: false,
        }
    }
}
impl PrintLayout {
    pub fn validate(&self) -> Result<(), String> {
        for v in [self.width_mm, self.height_mm, self.margin_mm, self.gap_mm] {
            if !v.is_finite() {
                return Err("Print dimensions must be finite".into());
            }
        }
        if !(25.0..=2000.0).contains(&self.width_mm)
            || !(25.0..=2000.0).contains(&self.height_mm)
            || !(1..=10).contains(&self.columns)
            || !(1..=10).contains(&self.rows)
            || !(72..=1200).contains(&self.ppi)
            || self.margin_mm < 0.0
            || self.gap_mm < 0.0
        {
            return Err("Invalid paper, grid, or resolution settings".into());
        }
        let (w, h) = self.cell_mm();
        if w < 5.0 || h < 5.0 {
            return Err("Margins and spacing leave no room for the photos".into());
        }
        Ok(())
    }
    pub fn cell_mm(&self) -> (f32, f32) {
        (
            (self.width_mm
                - 2.0 * self.margin_mm
                - self.gap_mm * self.columns.saturating_sub(1) as f32)
                / self.columns.max(1) as f32,
            (self.height_mm
                - 2.0 * self.margin_mm
                - self.gap_mm * self.rows.saturating_sub(1) as f32)
                / self.rows.max(1) as f32
                - if self.captions { 5.0 } else { 0.0 },
        )
    }
    pub fn image_pixels(&self) -> (u32, u32) {
        let (w, h) = self.cell_mm();
        (
            (w / 25.4 * self.ppi as f32).round().max(1.0) as u32,
            (h / 25.4 * self.ppi as f32).round().max(1.0) as u32,
        )
    }
}
struct Cell {
    image: u32,
    width: u32,
    height: u32,
    caption: String,
}
pub struct PrintDocument {
    file: tempfile::NamedTempFile,
    destination: PathBuf,
    layout: PrintLayout,
    offsets: Vec<u64>,
    pages: Vec<u32>,
    cells: Vec<Cell>,
    image_count: usize,
}
impl PrintDocument {
    pub fn new(
        destination: &Path,
        layout: PrintLayout,
        space: ExportColorSpace,
    ) -> Result<Self, String> {
        layout.validate()?;
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        let mut doc = Self {
            file,
            destination: destination.into(),
            layout,
            offsets: vec![0; 5],
            pages: Vec::new(),
            cells: Vec::new(),
            image_count: 0,
        };
        doc.file
            .write_all(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")
            .map_err(|e| e.to_string())?;
        doc.object(
            3,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
            None,
        )?;
        doc.object(
            4,
            "/N 3 /Alternate /DeviceRGB",
            Some(&crate::icc_profile(space).map_err(|e| e.to_string())?),
        )?;
        Ok(doc)
    }
    fn allocate(&mut self) -> u32 {
        let id = self.offsets.len() as u32;
        self.offsets.push(0);
        id
    }
    fn object(&mut self, id: u32, dict: &str, stream: Option<&[u8]>) -> Result<(), String> {
        self.offsets[id as usize] = self.file.stream_position().map_err(|e| e.to_string())?;
        writeln!(self.file, "{id} 0 obj").map_err(|e| e.to_string())?;
        if let Some(bytes) = stream {
            writeln!(self.file, "<< {dict} /Length {} >>\nstream", bytes.len())
                .map_err(|e| e.to_string())?;
            self.file.write_all(bytes).map_err(|e| e.to_string())?;
            self.file
                .write_all(b"\nendstream\nendobj\n")
                .map_err(|e| e.to_string())?;
        } else {
            writeln!(self.file, "{dict}\nendobj").map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// The caller supplies already prepared pixels in the document's RGB space.
    /// Only one encoded photo and one page's small placement records are held.
    pub fn push(&mut self, image: &ExportImage, caption: &str) -> Result<(), String> {
        if image.width == 0
            || image.height == 0
            || image.width > 65535
            || image.height > 65535
            || image.rgba8.len() as u64 != u64::from(image.width) * u64::from(image.height) * 4
        {
            return Err("Invalid print image dimensions".into());
        }
        let mut jpeg = Vec::new();
        let mut encoder = jpeg_encoder::Encoder::new(&mut jpeg, 98);
        encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
        encoder
            .encode(
                &image.rgb8(),
                image.width as u16,
                image.height as u16,
                jpeg_encoder::ColorType::Rgb,
            )
            .map_err(|e| e.to_string())?;
        let id = self.allocate();
        self.object(id,&format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace [/ICCBased 4 0 R] /BitsPerComponent 8 /Filter /DCTDecode",image.width,image.height),Some(&jpeg))?;
        self.cells.push(Cell {
            image: id,
            width: image.width,
            height: image.height,
            caption: caption.into(),
        });
        self.image_count += 1;
        if self.cells.len() >= self.layout.rows as usize * self.layout.columns as usize {
            self.flush_page()?;
        }
        Ok(())
    }
    fn flush_page(&mut self) -> Result<(), String> {
        if self.cells.is_empty() {
            return Ok(());
        }
        let pt = 72.0 / 25.4;
        let (wmm, hmm) = self.layout.cell_mm();
        let (w, h) = (wmm * pt, hmm * pt);
        let mut content = String::new();
        let mut resources = String::new();
        for (i, cell) in self.cells.iter().enumerate() {
            let column = i as u32 % self.layout.columns;
            let row = i as u32 / self.layout.columns;
            let x = (self.layout.margin_mm + column as f32 * (wmm + self.layout.gap_mm)) * pt;
            let y = (self.layout.height_mm
                - self.layout.margin_mm
                - row as f32
                    * (hmm + self.layout.gap_mm + if self.layout.captions { 5.0 } else { 0.0 })
                - hmm)
                * pt;
            let sx = w / cell.width as f32;
            let sy = h / cell.height as f32;
            let scale = if self.layout.fill {
                sx.max(sy)
            } else {
                sx.min(sy)
            };
            let iw = cell.width as f32 * scale;
            let ih = cell.height as f32 * scale;
            let ix = x + (w - iw) * 0.5;
            let iy = y + (h - ih) * 0.5;
            content.push_str(&format!("q {x:.4} {y:.4} {w:.4} {h:.4} re W n {iw:.4} 0 0 {ih:.4} {ix:.4} {iy:.4} cm /Im{} Do Q\n",cell.image));
            resources.push_str(&format!("/Im{} {} 0 R ", cell.image, cell.image));
            if self.layout.captions {
                let max_chars = (w / 4.5).floor().max(1.0) as usize;
                // Hex strings prevent PDF-syntax injection and encode the actual
                // WinAnsi font repertoire, including accented names and ©/€.
                let label: String = cell
                    .caption
                    .chars()
                    .take(max_chars)
                    .map(win_ansi)
                    .map(|byte| format!("{byte:02X}"))
                    .collect();
                content.push_str(&format!(
                    "BT /F1 8 Tf 0 g {x:.4} {:.4} Td <{label}> Tj ET\n",
                    y - 11.0
                ));
            }
        }
        let stream = self.allocate();
        self.object(stream, "", Some(content.as_bytes()))?;
        let page = self.allocate();
        self.object(page,&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.4} {:.4}] /Resources << /Font << /F1 3 0 R >> /XObject << {resources} >> >> /Contents {stream} 0 R >>",self.layout.width_mm*pt,self.layout.height_mm*pt),None)?;
        self.pages.push(page);
        self.cells.clear();
        Ok(())
    }
    pub fn finish(mut self) -> Result<PathBuf, String> {
        if self.image_count == 0 {
            return Err("Print document has no photos".into());
        }
        self.flush_page()?;
        let kids = self
            .pages
            .iter()
            .map(|id| format!("{id} 0 R"))
            .collect::<Vec<_>>()
            .join(" ");
        self.object(
            2,
            &format!(
                "<< /Type /Pages /Count {} /Kids [{kids}] >>",
                self.pages.len()
            ),
            None,
        )?;
        self.object(1, "<< /Type /Catalog /Pages 2 0 R >>", None)?;
        let xref = self.file.stream_position().map_err(|e| e.to_string())?;
        if xref >= 10_000_000_000 {
            return Err("Print PDF is too large".into());
        }
        writeln!(
            self.file,
            "xref\n0 {}\n0000000000 65535 f ",
            self.offsets.len()
        )
        .map_err(|e| e.to_string())?;
        for offset in &self.offsets[1..] {
            writeln!(self.file, "{offset:010} 00000 n ").map_err(|e| e.to_string())?;
        }
        writeln!(
            self.file,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
            self.offsets.len()
        )
        .map_err(|e| e.to_string())?;
        self.file.flush().map_err(|e| e.to_string())?;
        self.file.as_file().sync_all().map_err(|e| e.to_string())?;
        self.file
            .persist(&self.destination)
            .map_err(|e| e.to_string())?;
        Ok(self.destination)
    }
}
fn win_ansi(character: char) -> u8 {
    let code = character as u32;
    if (32..=126).contains(&code) || (160..=255).contains(&code) {
        return code as u8;
    }
    match character {
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        c if c.is_whitespace() => b' ',
        _ => b'?',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_grid_has_correct_resolution_and_rejects_impossible_margins() {
        let mut layout = PrintLayout::default();
        assert_eq!(layout.image_pixels(), (2244, 3272));
        layout.margin_mm = 150.0;
        assert!(layout.validate().is_err());
    }
    #[test]
    fn pdf_pages_have_physical_media_and_valid_references() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contact.pdf");
        let mut doc = PrintDocument::new(
            &path,
            PrintLayout {
                columns: 2,
                rows: 2,
                captions: true,
                ..Default::default()
            },
            ExportColorSpace::Srgb,
        )
        .unwrap();
        let image = ExportImage::new(20, 10, [100, 140, 190, 255].repeat(200)).unwrap();
        for _ in 0..5 {
            doc.push(&image, "Été © (safe) \\ €").unwrap();
        }
        doc.finish().unwrap();
        let pdf = lopdf::Document::load(&path).unwrap();
        let pages = pdf.get_pages();
        assert_eq!(pages.len(), 2);
        for (_, id) in pages {
            let page = pdf.get_dictionary(id).unwrap();
            let bounds = page.get(b"MediaBox").unwrap().as_array().unwrap();
            assert!((bounds[2].as_float().unwrap() - 595.2756).abs() < 0.01);
            assert!((bounds[3].as_float().unwrap() - 841.8898).abs() < 0.01);
            assert!(!pdf.get_page_content(id).unwrap().is_empty());
        }
        let captions = pdf.extract_text(&[1, 2]).unwrap();
        assert!(captions.contains("Été © (safe) \\ €"), "{captions}");
        let good = std::fs::read(&path).unwrap();
        assert!(
            PrintDocument::new(&path, PrintLayout::default(), ExportColorSpace::Srgb)
                .unwrap()
                .finish()
                .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), good);
    }
}
