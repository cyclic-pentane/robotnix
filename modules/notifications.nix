{ config, lib, pkgs, ... }: with lib;
let
  cfg = config.notifications;
in {
    options = {
	notifications.defaultVibrationPattern = mkOption {
	    type = types.listOf types.int;
	    default = [ 0 350 250 350 ];
	    description = ''
		The vibration pattern for notifications whose apps don't set custom vibration patterns. The list consists of an even number of durations in milliseconds. The vibration motor is turned on every second step. For instance, a value of `[ 0 350 250 350 ]` means that the motor is turned off for 0 milliseconds, then turned on for 350 milliseconds, turned off for 250 milliseconds, and finally turned on for 350 milliseconds again.
	    '';
	};
    };

    config = {
      source.dirs."frameworks/base".patches = [
        (pkgs.substituteAll {
          src = ./vibration-pattern-patch-template.patch;
          vibration_pattern_xml = concatStringsSep " " (builtins.map (x: "<item>${builtins.toString x}</item>") cfg.defaultVibrationPattern);
        })
      ];
    };
}
