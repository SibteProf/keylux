//! Representative mechanical RGB targets, checked against OpenRGB 1.0rc3's
//! published device table on 2026-09-30. These are compatibility references,
//! not a USB allow-list: OpenRGB does all hardware detection and transport.
//! IDs refer to the specified variants; Bluetooth support is not implied.

pub struct Model {
    pub name: &'static str,
    pub usb_ids: &'static str,
}

pub const MODELS: &[Model] = &[
    Model {
        name: "Corsair K100 RGB (MX Red)",
        usb_ids: "1b1c:1b7d",
    },
    Model {
        name: "Corsair K70 RGB PRO",
        usb_ids: "1b1c:1bc4, 1b1c:1bb3",
    },
    Model {
        name: "Corsair K95 RGB Platinum XT",
        usb_ids: "1b1c:1b89",
    },
    Model {
        name: "Razer BlackWidow V4 Pro",
        usb_ids: "1532:028d",
    },
    Model {
        name: "Razer BlackWidow V4 75%",
        usb_ids: "1532:02a5",
    },
    Model {
        name: "Razer BlackWidow V4 Pro 75% (wired)",
        usb_ids: "1532:02b3",
    },
    Model {
        name: "ASUS ROG Azoth (wired)",
        usb_ids: "0b05:1a83",
    },
    Model {
        name: "ASUS ROG Strix Scope II 96 Wireless (USB)",
        usb_ids: "0b05:1aae",
    },
    Model {
        name: "Logitech G915 (wired)",
        usb_ids: "046d:c33e",
    },
    Model {
        name: "Logitech G915 TKL (wired)",
        usb_ids: "046d:c343",
    },
    Model {
        name: "HyperX Alloy Origins",
        usb_ids: "0951:16e5, 03f0:0591",
    },
    Model {
        name: "HyperX Alloy Origins Core",
        usb_ids: "0951:16e6, 03f0:098f",
    },
    Model {
        name: "HyperX Alloy Origins 65",
        usb_ids: "03f0:038f",
    },
    Model {
        name: "HyperX Alloy Elite 2",
        usb_ids: "0951:1711, 03f0:058f",
    },
    Model {
        name: "ASUS ROG Strix Scope II",
        usb_ids: "0b05:1ab3",
    },
    // Magnetic/optical switch choices are extra targets, outside the 15
    // conventional mechanical models above.
    Model {
        name: "SteelSeries Apex Pro TKL Gen 3 (wired, Hall effect)",
        usb_ids: "1038:1642",
    },
    Model {
        name: "Razer Huntsman V3 Pro (analog optical)",
        usb_ids: "1532:02a6",
    },
];
