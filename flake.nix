{
	description = "Flake for dumbo";
	inputs = {
		nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
		rust-overlay.url = "github:oxalica/rust-overlay";
		flake-utils.url = "github:numtide/flake-utils";
		nix-kdl.url = "github:Lhcfl/nix-kdl";
	};

	outputs = {
		self,
		nixpkgs,
		rust-overlay,
		nix-kdl
	}: flake-utils.lib.eachDefaultSystem (system: let
		overlays = [(import rust-overlay)];
		pkgs = import nixpkgs {
			inherit system overlays;
		};

		rustToolchain = pkgs.rust-bin.stable.latest.minimal.override {
			extensions = [ "rust-src" ];
		};

		buildInputs = with pkgs; [
			rustToolchain
			clang
			mold
		];
		dumbo_pkg = pkgs.rustPlatform.buildRustPackage {
			name = "dumbo";
			src = ./.;
			cargoBuildFlags = [ "--release" ];
			cargoLock.lockFile = ./Cargo.lock;
			doCheck = false;
			inherit buildInputs;
			nativeBuildInputs = buildInputs;
		};
	in {
		packages = rec {
			default = dumbo;
			dumbo = dumbo_pkg;
		};
		nixosModules = rec {
			default = dumbo;
			dumbo = { config, ... }: let
				cfg = config.services.dumbo;
			in {
				options = {
					pkgs.dumbo = dumbo_pkg;
					services.dumbo = with lib; let
						opt = type: description: mkOption {
							type = type;
							description = description;
						};
					in {
						enable = mkxEnableOption "Enable dumbo daemon";

						ntfyURL = opt types.str "The full ntfy endpoint url";
						ntfyTopic = opt types.str "The ntfy topic";
						passiveMode = opt types.bool "If true, the daemon will never try to take any actions besides reporting issues";
						watchModules = opt
							(lib.types.attrsOf (lib.types.submodule (
								{ name, ... }:
								{
									options = {
										ensureLoaded = opt types.bool "If true, the daemon will check if the given module is loaded. If not, it will do no such checking";
										ensureUsedBy = opt
											(types.listOf (types.ints.between 0 65536))
											"PCI device IDs to ensure that this kernel module is being used by";
										onFailure = opt
											(lib.types.attrsOf (lib.types.submodule {
												tryModload = opt
													types.bool
													"If true, the daemon will try to `modload` the module when the module's not loaded but is expected to be";
												tryPciRescan = opt
													types.bool
													"If true, the daemon will cause the system to rescan the available PCI devices whenever an expected device is not using the module";

												reboot = opt
													types.bool
													"If true, reboot the computer upon failure of one of the previously-stated conditions";
											}))
											"What to do when any of the checks above fail";
									};
								}
							)))
							"Modules to watch";

						watchMountpoints = opt
							(lib.types.attrsOf (lib.types.submodule (
								{ device, ... }: {
									options = {
										mountPoint = opt types.str "The mount point that we expect this block device to be mounted to";
										fsType = opt types.str "The FS type that we expect this block device/mount point to have";
										options = opt (types.listOf types.str) "The mount options that we should try to use when re-mounting this upon encountering a failure";
									};
								}
							)))
							"Mountpoints to monitor/watch";

						ensureReadableFilesWithin = opt
							(types.listOf types.str)
							"Directories to ensure that we can read at least one file from within";
					};
				};
				config = lib.mkIf cfg.enable {
					systemd.services.dumbo = let
						val_to_kdl = name: val: with kdl.dsl; (n name (
							if (builtins.isAttrs val) then (
								builtins.mapAttrs val_to_kdl val
							) else (
								value
							)
						));
						kdl_data = [
							builtins.mapAttrs
								val_to_kdl
								(lib.filterAttrs
									# don't keep the enable attribute
									(name: _: name != "enable")
									cfg)
						];
						kdl_config_file = lib.writeText {
							name = "dumbo.kdl";
							text = (kdl.formats.v2 kdl_data);
						};
					in {
						description = "dumbo daemon for watching a few simple system conditions and warning whenever they are not met";
						wantedBy = [ "multi-user.target" ];
						# after = # figure out this one. Probably ntfy
						restartIfChanged = true;

						serviceConfig = {
							Type = "simple";
							Restart = "always";

							DynamicUser = true;
							ExecStart = "${dumbo_pkg}/bin/dumbo --config-file ${kdl_config_file}"; # pass in cli args
						};
					};
				};
			};
		};
	});
}
