use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn acquire() -> Result<InstanceGuard, String> {
    acquire_named("Local\\OptiScalerNR_Installer")
}

fn acquire_named(name: &str) -> Result<InstanceGuard, String> {
    let name = windows::core::HSTRING::from(name);
    let handle =
        unsafe { CreateMutexW(None, true, &name) }.map_err(|e| format!("创建单实例锁失败: {e}"))?;
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(handle) };
        return Err("OptiScaler NR Installer 已经在运行".to_string());
    }
    Ok(InstanceGuard(handle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_second_instance() {
        let name = format!(
            "Local\\OptiScalerNR_Test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let first = acquire_named(&name).unwrap();
        assert!(acquire_named(&name).is_err());
        drop(first);
    }
}
