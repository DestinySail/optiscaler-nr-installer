use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct PeInfo {
    pub is_64_bit: bool,
    pub imports: Vec<String>,
}

pub fn inspect(path: &Path) -> Result<PeInfo, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取 PE {} 失败: {e}", path.display()))?;
    inspect_bytes(&bytes)
}

fn inspect_bytes(bytes: &[u8]) -> Result<PeInfo, String> {
    let pe_offset = read_u32(bytes, 0x3c)? as usize;
    if bytes.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        return Err("不是 PE 文件".to_string());
    }
    let coff = pe_offset + 4;
    let sections = read_u16(bytes, coff + 2)? as usize;
    let optional_size = read_u16(bytes, coff + 16)? as usize;
    let optional = coff + 20;
    let magic = read_u16(bytes, optional)?;
    let is_64_bit = magic == 0x20b;
    let dirs = optional
        + if is_64_bit {
            112
        } else if magic == 0x10b {
            96
        } else {
            return Err("未知 PE 可选头".to_string());
        };
    let import_rva = read_u32(bytes, dirs + 8)? as usize;
    let import_size = read_u32(bytes, dirs + 12)? as usize;
    let section_table = optional + optional_size;
    let section_entries = (0..sections)
        .map(|index| {
            let base = section_table + index * 40;
            Ok(Section {
                virtual_address: read_u32(bytes, base + 12)? as usize,
                virtual_size: read_u32(bytes, base + 8)? as usize,
                raw_size: read_u32(bytes, base + 16)? as usize,
                raw_offset: read_u32(bytes, base + 20)? as usize,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut imports = Vec::new();
    if import_rva != 0 && import_size != 0 {
        let mut cursor = rva_to_offset(import_rva, &section_entries)
            .ok_or_else(|| "导入表 RVA 无效".to_string())?;
        for _ in 0..512 {
            let name_rva = read_u32(bytes, cursor + 12)? as usize;
            let thunk = read_u32(bytes, cursor)?;
            let first_thunk = read_u32(bytes, cursor + 16)?;
            if name_rva == 0 && thunk == 0 && first_thunk == 0 {
                break;
            }
            if name_rva != 0
                && let Some(offset) = rva_to_offset(name_rva, &section_entries)
                && let Some(end) = bytes[offset..].iter().position(|b| *b == 0)
            {
                let name = String::from_utf8_lossy(&bytes[offset..offset + end]).to_string();
                if !name.is_empty() {
                    imports.push(name);
                }
            }
            cursor += 20;
        }
    }
    imports.sort_by_key(|name| name.to_ascii_lowercase());
    imports.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Ok(PeInfo { is_64_bit, imports })
}

#[derive(Debug)]
struct Section {
    virtual_address: usize,
    virtual_size: usize,
    raw_size: usize,
    raw_offset: usize,
}

fn rva_to_offset(rva: usize, sections: &[Section]) -> Option<usize> {
    sections.iter().find_map(|section| {
        let span = section.virtual_size.max(section.raw_size);
        (rva >= section.virtual_address && rva < section.virtual_address + span)
            .then_some(section.raw_offset + rva - section.virtual_address)
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "PE 文件被截断".to_string())?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "PE 文件被截断".to_string())?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_rva_inside_section() {
        let sections = [Section {
            virtual_address: 0x1000,
            virtual_size: 0x300,
            raw_size: 0x200,
            raw_offset: 0x400,
        }];
        assert_eq!(rva_to_offset(0x1010, &sections), Some(0x410));
        assert_eq!(rva_to_offset(0x2000, &sections), None);
    }
}
