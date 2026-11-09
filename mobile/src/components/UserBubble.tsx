import { useState } from "react";
import { View } from "react-native";

import type { MessageAttachment } from "@sirus/client/types";

import { useMacAccent } from "../data/mac-accent";
import { SentAttachments } from "./SentAttachments";
import { UserText } from "./UserText";

/**
 * A sent message in the Mac's accent at 24%: a pill on one line, a card past it. The transcript,
 * an instruction steered into a running reply and a message waiting in the queue share it.
 */
export function UserBubble(props: { readonly text: string; readonly attachments?: readonly MessageAttachment[]; readonly computerId: string }) {
  const accent = useMacAccent(props.computerId);
  const [lines, setLines] = useState(1);
  const bubble = props.text.trim() ? <View style={{ alignSelf: "flex-end", maxWidth: "88%", paddingHorizontal: 14, paddingVertical: 9, borderRadius: lines > 1 ? 14 : 21, borderCurve: "continuous", backgroundColor: `${accent}3d` }}>
    <UserText text={props.text} onLines={setLines} />
  </View> : null;
  if (!props.attachments?.length) return bubble;
  // The files go over the bubble, as on the Mac (`sent_chips`).
  return <View className="items-end gap-1.5">
    <SentAttachments attachments={props.attachments} computerId={props.computerId} />
    {bubble}
  </View>;
}
