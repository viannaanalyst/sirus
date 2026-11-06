import { ActivityIndicator, Pressable, ScrollView, View } from "react-native";

import { AppText } from "../AppText";
import { FileIcon } from "../FileIcon";
import { SymbolView } from "../AppSymbol";
import type { Composer } from "./useComposer";
import type { Suggestion } from "./useComposerSuggestions";

const ROW_HEIGHT = 52;

function Row(props: { readonly row: Suggestion; readonly onPress: () => void }) {
  const { row } = props;
  return <Pressable onPress={props.onPress} accessibilityRole="button" accessibilityLabel={row.label} accessibilityHint={row.description || undefined}
    className="flex-row items-center gap-3 rounded-[14px] px-3 py-2 active:bg-row-hover" style={{ minHeight: ROW_HEIGHT - 6 }}>
    <View className="size-7 items-center justify-center">
      {row.path ? <FileIcon name={row.label.replace(/\/$/, "")} directory={row.directory} size={22} /> : <SymbolView name={"shippingbox" as never} size={20} tintColorClassName="accent-foreground-muted" />}
    </View>
    <View className="min-w-0 flex-1">
      <AppText numberOfLines={1} className="font-app-medium text-[15px]">{row.label}</AppText>
      {row.description ? <AppText numberOfLines={row.path ? 1 : 2} className="text-[12.5px] text-foreground-muted">{row.description}</AppText> : null}
    </View>
  </Pressable>;
}

/**
 * The `/` and `@` list, a card of the sheets' look right above the composer (and above the
 * keyboard, since the composer rides it). A tap on a row inserts the token.
 */
export function SuggestionsPanel(props: { readonly composer: Composer }) {
  const { suggestions, pick } = props.composer;
  if (!suggestions.visible) return null;
  const file = suggestions.kind === "file";
  const empty = !suggestions.rows.length && !suggestions.loading && !suggestions.error;
  return <View className="mb-2 overflow-hidden rounded-[22px] border border-border-subtle bg-sheet" accessibilityViewIsModal={false}>
    <View className="flex-row items-center justify-between px-4 pb-1 pt-2.5">
      <AppText className="text-[13px] text-foreground-muted">{file ? "Arquivos do projeto" : "Skills"}</AppText>
      {suggestions.loading ? <ActivityIndicator size="small" /> : null}
    </View>
    <ScrollView keyboardShouldPersistTaps="always" style={{ maxHeight: ROW_HEIGHT * 5 }} contentContainerClassName="px-1.5 pb-1.5" showsVerticalScrollIndicator={false}>
      {suggestions.error ? <View className="px-3 py-3">
        <AppText className="text-sm text-danger-foreground">{suggestions.error}</AppText>
        <Pressable onPress={suggestions.retry} accessibilityRole="button" className="mt-1.5 self-start active:opacity-60"><AppText className="font-app-medium text-sm text-adaptive-blue-600-400">Tentar de novo</AppText></Pressable>
      </View> : null}
      {suggestions.rows.map((row) => <Row key={row.key} row={row} onPress={() => pick(row)} />)}
      {empty ? <AppText className="px-3 py-3 text-sm text-foreground-muted">{file ? "Nenhum arquivo encontrado." : "Nenhuma skill encontrada."}</AppText> : null}
      {suggestions.truncated && suggestions.rows.length ? <AppText className="px-3 py-2 text-xs text-foreground-tertiary">Mostrando só os primeiros resultados. Continue digitando para filtrar.</AppText> : null}
    </ScrollView>
  </View>;
}
