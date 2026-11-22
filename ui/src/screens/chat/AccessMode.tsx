// Выбор режима доступа Помощника в нижней строке поля ввода. Разметка и режимы — по T3 Code (MIT):
// ComposerFooterControls.tsx (кнопка-значок и меню «Permissions» с переключателем режимов), runtimeModeConfig.ts лежит в src/t3 как есть.
// Окно показывает два режима: «Спрашивать разрешение» (approval-required) и «Полный доступ» (full-access).
import { Menu as MenuPrimitive } from '@base-ui/react/menu';
import { useEffect, useId, useState } from 'react';
import { CONFIRM_ACCESS_DESCRIPTION, CONFIRM_ACCESS_LEAD, FULL_ACCESS_DESCRIPTION } from '../../lib/executor.mjs';
import type { AccessMode } from './transport';
import { resolveRuntimeModeOption, type RuntimeModeOption } from './runtimeMode';

const OPTIONS: ReadonlyArray<AccessMode> = ['confirm', 'full'];
export const FULL_ACCESS_TEXT = 'Полный доступ: без вопросов';

export function AccessMode(props: { personalSettings: boolean; mode: AccessMode; busy: boolean; disabled: boolean; onChange: (mode: AccessMode) => void }) {
  const [preview, setPreview] = useState<AccessMode | null>(null);
  const [asking, setAsking] = useState(false);
  const descriptionId = useId();
  const shownMode = preview ?? props.mode;
  // The second phrase of «Спрашивать разрешение» only when the engine ran Claude with the person's own settings.
  const options = OPTIONS.map(mode => resolveRuntimeModeOption(mode)).map(option => option.mode === 'confirm' ? { ...option, description: props.personalSettings ? CONFIRM_ACCESS_DESCRIPTION : CONFIRM_ACCESS_LEAD } : option);
  const current = options.find(option => option.mode === props.mode) as RuntimeModeOption;
  const shown = options.find(option => option.mode === shownMode) as RuntimeModeOption;
  const chipText = props.mode === 'full' ? FULL_ACCESS_TEXT : current.label;
  useEffect(() => {
    if (!asking) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') setAsking(false); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [asking]);
  return (
    <>
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger
        className="chat-pick chat-access" data-chat-access data-access-mode={props.mode} data-busy={props.busy ? 'true' : 'false'}
        disabled={props.disabled || props.busy} aria-label={`Доступ: ${chipText}`} title="Режим доступа Помощника"
      >
        <current.Icon aria-hidden="true" className="ic ic-sm" />
        <span className="chat-access-text">{chipText}</span>
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner side="top" align="start" sideOffset={8}>
          <MenuPrimitive.Popup className="chat-access-menu" data-access-menu>
            <div className="chat-access-head">{'Доступ Помощника'}</div>
            <MenuPrimitive.RadioGroup
              aria-label="Доступ" value={props.mode} className="chat-access-group"
              onValueChange={(value: AccessMode) => { if (value === props.mode) return; if (value === 'full') setAsking(true); else props.onChange(value); }}
              onKeyDown={event => {
                if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
                const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="menuitemradio"]'));
                const index = items.findIndex(item => item === document.activeElement);
                const next = items[index + (event.key === 'ArrowRight' ? 1 : -1)];
                if (index < 0 || !next) return;
                event.preventDefault(); event.stopPropagation(); next.focus();
              }}
            >
              {options.map(option => (
                <MenuPrimitive.RadioItem
                  key={option.mode} value={option.mode} label={option.label} aria-label={option.label} data-access-option={option.mode}
                  aria-describedby={`${descriptionId}-${option.mode}`} closeOnClick className="chat-access-item"
                  onPointerEnter={() => setPreview(option.mode)} onPointerLeave={() => setPreview(null)}
                  onFocus={() => setPreview(option.mode)} onBlur={() => setPreview(null)}
                >
                  <option.Icon aria-hidden="true" className="ic ic-sm" />
                  <span>{option.label}</span>
                </MenuPrimitive.RadioItem>
              ))}
            </MenuPrimitive.RadioGroup>
            {options.map(option => <span key={option.mode} id={`${descriptionId}-${option.mode}`} className="chat-sr-only">{option.description}</span>)}
            <div className="chat-access-preview" aria-hidden="true">
              <span data-active={shownMode === props.mode ? 'true' : 'false'}>{shown.label}</span>
              <span>{shown.description}</span>
            </div>
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
    {asking && (
      <div className="chat-dialog-backdrop" data-full-access-backdrop>
        <div className="chat-dialog" role="alertdialog" aria-modal="true" aria-labelledby={`${descriptionId}-title`} aria-describedby={`${descriptionId}-text`} data-full-access-dialog>
          <h2 id={`${descriptionId}-title`}>Включить полный доступ?</h2>
          <p id={`${descriptionId}-text`} className="chat-dialog-text">Помощник перестанет спрашивать разрешение. {FULL_ACCESS_DESCRIPTION} Вернуть вопросы можно в любой момент тем же переключателем.</p>
          <div className="chat-dialog-foot">
            <button type="button" className="btn" data-enable-full onClick={() => { setAsking(false); props.onChange('full'); }}>Включить полный доступ</button>
            <button type="button" className="btn btn-primary" data-keep-questions autoFocus onClick={() => setAsking(false)}>Оставить вопросы</button>
          </div>
        </div>
      </div>
    )}
    </>
  );
}
