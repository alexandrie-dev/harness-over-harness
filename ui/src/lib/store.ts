import { create } from 'zustand';
import { ChatController, type ChatState } from '../screens/chat/controller.mjs';
import { chatTransport, type ChatTransport } from '../screens/chat/transport';

export type { ChatState } from '../screens/chat/controller.mjs';
export function createChatStore(transportFactory: () => ChatTransport = chatTransport) {
  return create<ChatState>(set => {
    const controller = new ChatController(transportFactory, state => set(state));
    return {
      ...controller.state,
      setDraft: value => controller.setDraft(value),
      connect: () => controller.connect(),
      disconnect: () => controller.disconnect(),
      send: () => controller.send(),
      stop: () => controller.stop(),
    };
  });
}
export const useChatStore = createChatStore();
