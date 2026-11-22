// Выбор модели Claude в нижней строке поля ввода. Разметка по ProviderModelPicker T3 Code (MIT): кнопка со значком
// поставщика и именем модели, список моделей с отметкой выбранной. Сам файл T3 завязан на Effect-хранилища, поповеры и
// ключевые сочетания T3 и в окно не переносится; здесь та же разметка на меню base-ui, как у переключателя доступа.
import { Menu as MenuPrimitive } from '@base-ui/react/menu';
import { claudeModelLabel } from '../../lib/executor.mjs';
import { Icon } from '../shared/icons';
import { ProviderMark } from '../shared/ProviderMark';

export interface ExecutorModelPickerProps {
  /** The model chosen for NEW chats ('' when new chats do not go through Claude); `model` is the model of the open chat. */
  newModel: string;
  model: string;
  models: { id: string; name: string }[];
  busy: boolean;
  disabled: boolean;
  onOpen(): void;
  onPick(id: string): void;
  onChangeProvider(): void;
}

export function ExecutorModelPicker({ model, newModel, models, busy, disabled, onOpen, onPick, onChangeProvider }: ExecutorModelPickerProps) {
  const label = claudeModelLabel(model, models);
  const listed = !newModel || models.some(item => item.id === newModel) ? models : [{ id: newModel, name: claudeModelLabel(newModel, models) }, ...models];
  return (
    <MenuPrimitive.Root onOpenChange={open => { if (open) onOpen(); }}>
      <MenuPrimitive.Trigger
        className="chat-pick chat-pick-model" data-chat-model-chip data-executor="claude" data-busy={busy ? 'true' : 'false'}
        disabled={disabled || busy} aria-label={`Модель Claude: ${label}`} title={`Claude: ${label}`}
      >
        <ProviderMark icon="claude" /><span data-chat-model>{label}</span>
      </MenuPrimitive.Trigger>
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner side="top" align="start" sideOffset={8}>
          <MenuPrimitive.Popup className="chat-access-menu" data-model-menu>
            <div className="chat-access-head">Модель Claude для новых чатов</div>
            <div className="chat-model-fact" data-new-chats-line>Этот чат остаётся на модели «{label}».</div>
            <MenuPrimitive.RadioGroup aria-label="Модель Claude" value={newModel} className="chat-model-group" onValueChange={(value: string) => { if (value !== newModel) onPick(value); }}>
              {listed.map(item => (
                <MenuPrimitive.RadioItem key={item.id} value={item.id} label={item.name} data-model-option={item.id} closeOnClick className="chat-model-item">
                  <ProviderMark icon="claude" /><span>{item.name}</span>
                  <MenuPrimitive.RadioItemIndicator className="chat-model-check"><Icon name="check" small /></MenuPrimitive.RadioItemIndicator>
                </MenuPrimitive.RadioItem>
              ))}
            </MenuPrimitive.RadioGroup>
            <MenuPrimitive.Item className="chat-model-item chat-model-change" data-change-provider onClick={onChangeProvider}>Сменить поставщика…</MenuPrimitive.Item>
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
