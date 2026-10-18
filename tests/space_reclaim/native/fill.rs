//! Filling the isolated volume past the product's own disk-full rule.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::Native;

/// One percent is one hundredth, as the SI brochure defines the symbol `%`:
/// https://www.bipm.org/en/publications/si-brochure
const PERCENT: u64 = 100;

impl Native {
    /// The disk section `stado host gates` reports for this host: the
    /// product's own reading of the isolated volume against its rule. Gates
    /// exit non-zero while any blocker stands, so the report is read from
    /// stdout.
    pub fn disk(&self) -> Value {
        let output = self.run(&["host", "gates", "example-cleanup-host", "--json"]);
        let report: Value =
            serde_json::from_slice(&output.stdout).expect("host gates printed JSON");
        report["disk"].clone()
    }

    /// Fill the test volume with user data past the threshold the product
    /// declares, in blocks of the volume's own allocation size, until the
    /// volume keeps less free than the rule's reserve; the product's own
    /// reading then has to call it full.
    pub fn fill_with_user_data(&self) -> PathBuf {
        let documents = self.home.join("Documents");
        fs::create_dir_all(&documents).unwrap();
        let path = documents.join("user-data.bin");
        let full_percent = self.disk()["full_percent"]
            .as_u64()
            .expect("the product reports its disk-full threshold");
        let total = fs2::total_space(&self.home).unwrap();
        let reserve = total / PERCENT * (PERCENT - full_percent);
        let granularity = fs2::allocation_granularity(&self.home).unwrap();
        let block = vec![u8::MAX; usize::try_from(granularity).unwrap()];
        let mut file = File::create(&path).unwrap();
        loop {
            let free = fs2::available_space(&self.home).unwrap();
            if free < reserve {
                break;
            }
            // Past the reserve by one block, written before free space is
            // read again.
            let blocks = (free - reserve + granularity).div_ceil(granularity);
            for block in std::iter::repeat_n(&block, usize::try_from(blocks).unwrap()) {
                file.write_all(block).unwrap();
            }
            file.sync_all().unwrap();
        }
        let disk = self.disk();
        self.observe(
            "user data",
            json!({"path": path, "bytes": file.metadata().unwrap().len(), "disk": disk}),
        );
        assert_eq!(
            disk["full"], true,
            "the product does not read the test volume as full: {disk}"
        );
        path
    }
}
