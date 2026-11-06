import { useMemo, useState } from "react";
import { Image, Pressable, View } from "react-native";

import type { MessageAttachment } from "@sirus/client/types";
import { splitAttachments } from "@sirus/lib/sent-attachments";

import { useSirus } from "../data/store";
import { SymbolView } from "./AppSymbol";
import { AppText } from "./AppText";
import { ImageViewer } from "./ImageViewer";

const SIDE = 96;

/**
 * What a sent message attached, over its bubble: pictures as tappable thumbnails (a tap opens
 * them full screen), other files as chips (the Mac's `sent_chips`).
 */
export function SentAttachments(props: { readonly attachments: readonly MessageAttachment[]; readonly computerId: string }) {
  const [open, setOpen] = useState<number | null>(null);
  const { images, files } = useMemo(() => {
    const transport = useSirus.getState().transportFor(props.computerId);
    return splitAttachments(props.attachments, (id) => transport?.thumbnailUrl?.(id));
  }, [props.attachments, props.computerId]);
  return <View className="max-w-[88%] flex-row flex-wrap justify-end gap-1.5 self-end">
    {images.map((image, index) => <Pressable key={image.key} onPress={() => setOpen(index)} accessibilityRole="imagebutton" accessibilityLabel={`Abrir ${image.name}`} className="active:opacity-80">
      <Image source={{ uri: image.small }} style={{ width: SIDE, height: SIDE, borderRadius: 16 }} className="bg-card" resizeMode="cover" />
    </Pressable>)}
    {files.map((file, index) => <View key={`${file.id ?? file.name}:${index}`} className="h-8 max-w-[200px] flex-row items-center gap-1.5 rounded-full bg-card px-2.5">
      <SymbolView name={file.kind === "folder" ? "folder" : "doc"} size={13} tintColorClassName="accent-foreground-muted" />
      <AppText numberOfLines={1} className="shrink text-xs">{file.name}</AppText>
    </View>)}
    <ImageViewer uris={images.map((image) => image.full)} index={open} onClose={() => setOpen(null)} />
  </View>;
}
