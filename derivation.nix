{ pkgs, rustPlatform, stdenv }:

rustPlatform.buildRustPackage rec {
  name = "cerebro-${version}";
  version = "1.0.0";
  src = ./.;

  nativeBuildInputs = with pkgs; [
    pkg-config
  ];

  buildInputs = with pkgs; [
    lm_sensors
    fuse
  ];

  checkPhase = "";
  cargoSha256 = "sha256:1ylhvrbjdzrlhgvpza790z3cxlqch0ndxc4jxbkd3w38bi5mlgyv";

  meta = with stdenv.lib; {
    description = "System monitoring daemon";
    homepage = https://bitbucket.org/mathieuhardy/cerebro-rs;
    licence = licenses.isc;
    maintainers = [ maintainers.tailhook ];
    platforms = platforms.all;
  };
}
