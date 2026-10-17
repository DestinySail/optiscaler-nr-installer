use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

pub struct InstanceGuard(HANDLE);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn acquire() -> Result<InstanceGuard, String> {
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\OptiScalerNR_Installer")) }
        .map_err(|e| format!("创建单实例锁失败: {e}"))?;
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
        let first = acquire().unwrap();
        assert!(acquire().is_err());
        drop(first);
    }
}
