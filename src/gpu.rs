use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

use crate::model::{GpuClass, GpuInfo};

pub fn detect_gpus() -> Result<Vec<GpuInfo>, String> {
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| format!("创建 DXGI Factory 失败: {e}"))?;
    let mut result = Vec::new();
    for index in 0u32.. {
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        let desc =
            unsafe { adapter.GetDesc1() }.map_err(|e| format!("读取显卡 {index} 信息失败: {e}"))?;
        let name = String::from_utf16_lossy(
            &desc.Description[..desc
                .Description
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(desc.Description.len())],
        );
        let software = desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0;
        let mochizuki_class = if software {
            GpuClass::Software
        } else {
            classify(desc.VendorId, desc.DeviceId, &name)
        };
        result.push(GpuInfo {
            name,
            vendor_id: desc.VendorId,
            device_id: desc.DeviceId,
            dedicated_vram: desc.DedicatedVideoMemory as u64,
            luid: ((desc.AdapterLuid.HighPart as u64) << 32) | desc.AdapterLuid.LowPart as u64,
            software,
            mochizuki_class,
        });
    }
    if result.is_empty() {
        return Err("DXGI 没有返回显卡".to_string());
    }
    Ok(result)
}

fn classify(vendor_id: u32, device_id: u32, name: &str) -> GpuClass {
    if vendor_id != 0x1002 {
        return GpuClass::Other;
    }
    // RDNA4 discrete: Navi 48 (0x7550/0x7551) and Navi 44 (0x7590).
    let known_id = matches!(device_id, 0x7550 | 0x7551 | 0x7590);
    let known_name = ["RX 9070", "RX 9060"]
        .iter()
        .any(|needle| name.to_ascii_uppercase().contains(needle));
    if known_id || known_name {
        GpuClass::Rdna4
    } else {
        GpuClass::OtherAmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdna4_matching_is_narrow() {
        assert_eq!(
            classify(0x1002, 0x7550, "AMD Radeon RX 9070 XT"),
            GpuClass::Rdna4
        );
        assert_eq!(
            classify(0x1002, 0x7590, "AMD Radeon RX 9060 XT"),
            GpuClass::Rdna4
        );
        assert_eq!(
            classify(0x1002, 0x744c, "AMD Radeon RX 7900 XTX"),
            GpuClass::OtherAmd
        );
        assert_eq!(
            classify(0x10de, 0x2684, "NVIDIA GeForce RTX 4090"),
            GpuClass::Other
        );
    }
}
