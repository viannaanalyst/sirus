import { useState } from "react";
import { Image, Modal, Pressable, ScrollView, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { offerSaveImage } from "../data/save-image";
import { SymbolView } from "./AppSymbol";
import { AppText } from "./AppText";

/**
 * A full-screen viewer for pictures: swipe between them, pinch to zoom, the close button or a
 * downward fling of the safe area to leave; a long press offers to save the picture to Photos. `uris` are full-size addresses.
 */
export function ImageViewer(props: { readonly uris: readonly string[]; readonly index: number | null; readonly onClose: () => void }) {
  const { width, height } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const [page, setPage] = useState<number | null>(null);
  const start = props.index ?? 0;
  const shown = page ?? start;
  return <Modal visible={props.index !== null} transparent animationType="fade" onRequestClose={props.onClose} onDismiss={() => setPage(null)} statusBarTranslucent>
    <View style={{ flex: 1, backgroundColor: "#000" }}>
      <ScrollView horizontal pagingEnabled showsHorizontalScrollIndicator={false} contentOffset={{ x: start * width, y: 0 }}
        onMomentumScrollEnd={(event) => setPage(Math.round(event.nativeEvent.contentOffset.x / width))}>
        {props.uris.map((uri) => <ScrollView key={uri} style={{ width, height }} contentContainerStyle={{ width, height, alignItems: "center", justifyContent: "center" }}
          maximumZoomScale={4} minimumZoomScale={1} centerContent showsHorizontalScrollIndicator={false} showsVerticalScrollIndicator={false} bouncesZoom>
          <Pressable onLongPress={() => offerSaveImage(uri)} delayLongPress={350} accessibilityHint="Segure para salvar em Fotos">
            <Image source={{ uri }} style={{ width, height }} resizeMode="contain" accessibilityLabel="Imagem enviada" />
          </Pressable>
        </ScrollView>)}
      </ScrollView>
      <View pointerEvents="box-none" style={{ position: "absolute", top: insets.top + 8, left: 16, right: 16, flexDirection: "row", alignItems: "center", justifyContent: "space-between" }}>
        <Pressable onPress={props.onClose} accessibilityRole="button" accessibilityLabel="Fechar imagem" hitSlop={8} className="size-11 items-center justify-center rounded-full bg-[#ffffff26] active:opacity-60">
          <SymbolView name="xmark" size={18} weight="semibold" tintColor="#ffffff" />
        </Pressable>
        {props.uris.length > 1 ? <AppText className="text-sm tabular-nums text-white">{shown + 1} / {props.uris.length}</AppText> : <View />}
      </View>
    </View>
  </Modal>;
}
