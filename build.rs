use winres;

fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("ns.ico"); // path to your .ico
    res.set("FileDescription", "License Monitor - Neilsoft");
    res.set("ProductName", "LicenseMonitor");
    res.set("CompanyName", "Neilsoft");
    res.set("LegalCopyright", "© 2025 Neilsoft");
    res.set("FileVersion", "1.0");
    res.set("ProductVersion", "1.0");
    res.compile().unwrap();
}
