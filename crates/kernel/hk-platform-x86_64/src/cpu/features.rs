//! CPUID 特性矩阵。
//!
//! 启动期探测一次，供 FPU、SMEP/SMAP、XSAVE、RDRAND 等路径判断能力，
//! 避免各处重复写 CPUID 叶子与位偏移。

/// 探测到的 CPU 特性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// NX（EFER.NXE）。
    pub nx: bool,
    /// SSE。
    pub sse: bool,
    /// FXSAVE/FXRSTOR。
    pub fxsr: bool,
    /// XSAVE/XRSTOR 指令。
    pub xsave: bool,
    /// OSXSAVE（XCR0 可用）。
    pub osxsave: bool,
    /// AVX。
    pub avx: bool,
    /// AVX2。
    pub avx2: bool,
    /// RDRAND。
    pub rdrand: bool,
    /// RDSEED。
    pub rdseed: bool,
    /// SMEP。
    pub smep: bool,
    /// SMAP。
    pub smap: bool,
    /// UMIP。
    pub umip: bool,
    /// PCID。
    pub pcid: bool,
    /// 1 GiB 大页。
    pub gb_pages: bool,
    /// TSC deadline 定时器。
    pub tsc_deadline: bool,
    /// x2APIC。
    pub x2apic: bool,
    /// FSGSBASE。
    pub fsgsbase: bool,
}

impl Features {
    /// 探测当前 CPU。
    pub fn detect() -> Self {
        let leaf1 = core::arch::x86_64::__cpuid(1);
        let leaf7 = core::arch::x86_64::__cpuid_count(7, 0);
        let extended = core::arch::x86_64::__cpuid(0x8000_0001);

        let ecx1 = leaf1.ecx;
        let edx1 = leaf1.edx;
        let ebx7 = leaf7.ebx;
        let ecx7 = leaf7.ecx;
        let edx_ext = extended.edx;

        Self {
            nx: edx_ext & (1 << 20) != 0,
            sse: edx1 & (1 << 25) != 0,
            fxsr: edx1 & (1 << 24) != 0,
            xsave: ecx1 & (1 << 26) != 0,
            osxsave: ecx1 & (1 << 27) != 0,
            avx: ecx1 & (1 << 28) != 0,
            avx2: ebx7 & (1 << 5) != 0,
            rdrand: ecx1 & (1 << 30) != 0,
            rdseed: ebx7 & (1 << 18) != 0,
            smep: ebx7 & (1 << 7) != 0,
            smap: ebx7 & (1 << 20) != 0,
            umip: ecx7 & (1 << 2) != 0,
            pcid: ecx1 & (1 << 17) != 0,
            gb_pages: edx_ext & (1 << 26) != 0,
            tsc_deadline: ecx1 & (1 << 24) != 0,
            x2apic: ecx1 & (1 << 21) != 0,
            fsgsbase: ebx7 & 1 != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_x86_64_baseline() {
        let features = Features::detect();
        assert!(
            features.sse && features.fxsr,
            "x86_64 基线必须包含 SSE/FXSR"
        );
        assert!(features.nx, "x86_64 基线必须包含 NX");
    }
}
