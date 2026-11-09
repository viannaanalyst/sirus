import { useEffect, useState } from "react";
import { ActivityIndicator, Image, Pressable, View } from "react-native";

import { translate } from "@sirus/i18n";

import { offerSaveImage } from "../data/save-image";
import { ImageViewer } from "./ImageViewer";

const t = (key: string) => translate("pt-BR", key);

/** A link a phone can load on its own: a web address or inline data. Anything else is a file on the Mac. */
export const isDirectImage = (url: string) => /^(https?:|data:image\/)/i.test(url.trim());

/**
 * A picture in a reply, on its own line. A web address or inline picture loads directly; a path on
 * the computer is asked of it (`load`, the host's `reply_image`, which answers a data URI). A tap
 * opens it full screen, a long press offers "Salvar em Fotos".
 */
export function ReplyImage(props: { readonly url: string; readonly alt: string; readonly load?: (link: string) => Promise<string> }) {
  const { url, load } = props;
  const [uri, setUri] = useState<string | null>(isDirectImage(url) ? url.trim() : null);
  const [ratio, setRatio] = useState(4 / 3);
  const [open, setOpen] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    setFailed(false);
    if (isDirectImage(url)) { setUri(url.trim()); return; }
    setUri(null);
    if (!load) return;
    let live = true;
    load(url).then((data) => live && setUri(data), () => live && setFailed(true));
    return () => { live = false; };
  }, [url, load]);
  if (failed) return null;
  return <View className="mb-3 mt-1 self-start" style={{ maxWidth: "100%" }}>
    {uri ? <Pressable onPress={() => setOpen(true)} onLongPress={() => offerSaveImage(uri)} delayLongPress={350} accessibilityRole="imagebutton" accessibilityLabel={props.alt || t("reply.image")} accessibilityHint="Segure para salvar em Fotos">
      <Image source={{ uri }} resizeMode="contain" style={{ width: 280, maxWidth: "100%", aspectRatio: ratio, borderRadius: 10 }} className="bg-card"
        onLoad={(event) => { const { width, height } = event.nativeEvent.source; if (width && height) setRatio(width / height); }} onError={() => setFailed(true)} />
    </Pressable> : <View className="h-24 w-40 items-center justify-center rounded-[10px] bg-card"><ActivityIndicator /></View>}
    {uri ? <ImageViewer uris={[uri]} index={open ? 0 : null} onClose={() => setOpen(false)} /> : null}
  </View>;
}
