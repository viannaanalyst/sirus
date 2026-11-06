import { Platform, Switch } from "react-native";

import { useMacAccent } from "../data/mac-accent";
import { MaterialSwitch } from "./MaterialSwitch";
import type { ThemedSwitchProps } from "./MaterialSwitch.types";

export type { ThemedSwitchProps } from "./MaterialSwitch.types";

export function ThemedSwitch(props: ThemedSwitchProps) {
  const accent = useMacAccent();
  if (Platform.OS === "android") {
    return <MaterialSwitch {...props} />;
  }

  // Sirus palette: on takes the Mac's accent with a white knob, so it never shows the system blue.
  return (
    <Switch
      {...props}
      ios_backgroundColorClassName="accent-switch-inactive-track"
      trackColorOffClassName="accent-switch-inactive-track"
      trackColor={{ true: accent }}
      thumbColor={props.value ? "#ffffff" : undefined}
    />
  );
}
