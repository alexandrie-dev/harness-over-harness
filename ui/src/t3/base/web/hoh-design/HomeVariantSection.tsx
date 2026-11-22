// Settings → Appearance → «Home variant: A / B / C» (HOH-41 design round, temporary): the variant of the home screen
// and of the notch. Their segmented control, their row. Nothing is shown when the round is off (features.ts).
import { useEffect, useState } from "react";
import { SettingsRow, SettingsSection } from "../components/settings/settingsLayout";
import { Toggle, ToggleGroup } from "../components/ui/toggle-group";
import { HOH_DESIGN_ROUND } from "../../../../t3-entry/features";
import {
  HOME_VARIANTS, readHomeVariant, subscribeHomeVariant, writeHomeVariant, type HomeVariant,
} from "../../../../t3-entry/notch/variant";

export function HomeVariantSection() {
  const [variant, setVariant] = useState<HomeVariant>(readHomeVariant);
  useEffect(() => subscribeHomeVariant(setVariant), []);
  if (!HOH_DESIGN_ROUND) return null;
  return (
    <SettingsSection id="appearance-home-variant" title="Design round">
      <SettingsRow
        title="Home variant"
        description="Which of the three home screens and notches to show (Ctrl+Alt+1 / 2 / 3). Temporary."
        control={
          <ToggleGroup
            aria-label="Home variant"
            value={[variant]}
            onValueChange={(next) => {
              const value = next[0] as HomeVariant | undefined;
              if (value && HOME_VARIANTS.includes(value)) writeHomeVariant(value);
            }}
          >
            {HOME_VARIANTS.map((name) => (
              <Toggle key={name} value={name}>
                {name.toUpperCase()}
              </Toggle>
            ))}
          </ToggleGroup>
        }
      />
    </SettingsSection>
  );
}
