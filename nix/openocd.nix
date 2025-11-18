{
  openocd,
  fetchFromGitHub,
  autoreconfHook,
  pkg-config,
  libtool,
  which,
  git,
  libusb1,
  libftdi1,
  hidapi,
  libgpiod,
  jimtcl,
  libjaylink,
}:
openocd.overrideAttrs (prev: {
  src = fetchFromGitHub {
    owner = "openocd-org";
    repo = "openocd";
    rev = "4e78563a0083d32a7cbb3ebfa92f7b9142637bf3";
    hash = "sha256-IIuAJ84LWxRGh/rE7Fl1pD8GYpFobXOMDoWdZp70QUU=";
  };

  nativeBuildInputs = [
    autoreconfHook
    pkg-config
    libtool
    which
    git
  ];

  buildInputs = [
    libusb1
    libftdi1
    hidapi
    libgpiod
    jimtcl
    libjaylink
  ];

  configureFlags = [
    "--disable-werror"
    "--enable-aice"
    "--enable-amtjtagaccel"
    "--enable-armjtagew"
    "--enable-at91rm9200"
    "--enable-bcm2835gpio"
    "--enable-buspirate"
    "--enable-capstone"
    "--enable-cmsis-dap"
    "--enable-dummy"
    "--enable-ep93xx"
    "--enable-ftdi"
    "--enable-gw16012"
    "--enable-ioutil"
    "--enable-jlink"
    "--enable-jtag_vpi"
    "--enable-legacy-ft2232_libftdi"
    "--enable-oocd_trace"
    "--enable-opendous"
    "--enable-openjtag_ftdi"
    "--enable-osbdm"
    "--enable-parport"
    "--enable-presto_libftdi"
    "--enable-remote-bitbang"
    "--enable-rlink"
    "--enable-stlink"
    "--enable-sysfsgpio"
    "--enable-ti-icdi"
    "--enable-ulink"
    "--enable-usb-blaster-2"
    "--enable-usb_blaster_libftdi"
    "--enable-usbprog"
    "--enable-vsllink"
    "--enable-xlnx_pcie_xvc"
  ];
})
