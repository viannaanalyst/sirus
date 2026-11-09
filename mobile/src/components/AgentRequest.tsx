import { useState } from "react";
import { ActivityIndicator, Alert, Pressable, ScrollView, View } from "react-native";

import type { AgentResponse, PendingRequest } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { canAnswer } from "@sirus/lib/agent-activity";
import { providerById } from "@sirus/lib/provider-registry";
import { maskInput, maskSecrets } from "@sirus/lib/redact";

import { useSirus, type QueuedPrompt } from "../data/store";
import { SymbolView, type AppSymbolName } from "./AppSymbol";
import { AppText, AppTextInput } from "./AppText";
import { UserBubble } from "./UserBubble";

const t = (key: string) => translate("pt-BR", key);
const errorText = (reason: unknown) => reason instanceof Error ? reason.message : String(reason);

/**
 * An agent waiting on the person, as the desktop's request card (`RequestCard`): an approval
 * (command, file change or tool) with allow once, decline and cancel the turn, or the agent's
 * questions, one at a time.
 */
export function AgentRequest(props: { readonly computerId: string; readonly sessionId: string; readonly agent: string; readonly request: PendingRequest }) {
  const { request } = props;
  const [busy, setBusy] = useState<string | null>(null);
  const [step, setStep] = useState(0);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const questions = request.kind.type === "userInput" ? request.kind.questions : [];
  const question = questions[step];
  const send = async (id: string, response: AgentResponse) => {
    if (busy) return;
    setBusy(id);
    try {
      await useSirus.getState().respondRequest(props.computerId, props.sessionId, request, response);
    } catch (reason) {
      setBusy(null);
      Alert.alert(t("Response could not be sent. Check the session status and stop the turn if needed."), errorText(reason));
    }
  };
  const answer = () => {
    if (!question || !canAnswer(question, answers[question.id])) return;
    if (step < questions.length - 1) { setStep(step + 1); return; }
    void send("answer", { type: "userInput", answers: Object.fromEntries(questions.map((item) => [item.id, [answers[item.id] ?? ""]])) });
  };
  const provider = (() => { try { return providerById(props.agent as never).name; } catch { return props.agent; } })();
  const kind = request.kind;

  return <View className="gap-3 rounded-2xl border border-border-subtle bg-card p-4">
    <View className="flex-row items-center gap-1.5">
      <SymbolView name={(kind.type === "userInput" ? "bubble.left" : "checkmark.shield") as AppSymbolName} size={13} tintColorClassName="accent-foreground-muted" />
      <AppText className="flex-1 text-[13px] text-foreground-muted">{provider} · {t(kind.type === "userInput" ? "Your response is needed" : "Approval requested")}</AppText>
      {questions.length > 1 ? <AppText className="text-[13px] text-foreground-muted">{step + 1}/{questions.length}</AppText> : null}
    </View>
    {kind.type === "userInput" ? question ? <View className="gap-2">
      <AppText className="font-app-medium text-base">{question.question}</AppText>
      {question.header ? <AppText className="text-[13px] text-foreground-muted">{question.header}</AppText> : null}
      {question.options?.map((option) => {
        const chosen = answers[question.id] === option.label;
        return <Pressable key={option.label} onPress={() => setAnswers((current) => ({ ...current, [question.id]: option.label }))}
          className={`flex-row items-start gap-2.5 rounded-xl border px-3 py-2.5 active:opacity-70 ${chosen ? "border-foreground" : "border-border-subtle"}`}>
          <SymbolView name={(chosen ? "largecircle.fill.circle" : "circle") as AppSymbolName} size={16} tintColorClassName={chosen ? "accent-foreground" : "accent-foreground-muted"} />
          <View className="flex-1">
            <AppText className="text-[15px]">{option.label}</AppText>
            {option.description ? <AppText className="text-[13px] text-foreground-muted">{option.description}</AppText> : null}
          </View>
        </Pressable>;
      })}
      {!question.options || question.isOther ? <AppTextInput value={answers[question.id] ?? ""} onChangeText={(value) => setAnswers((current) => ({ ...current, [question.id]: value }))}
        placeholder={t("Your answer")} secureTextEntry={question.isSecret} maxLength={8192} className="text-base" /> : null}
      <View className="flex-row items-center justify-between pt-1">
        {step > 0 ? <Button label={t("Back")} onPress={() => setStep(step - 1)} disabled={!!busy} /> : <View />}
        <Button primary label={t(step < questions.length - 1 ? "Continue" : "Send answer")} busy={busy === "answer"} disabled={!!busy || !canAnswer(question, answers[question.id])} onPress={answer} />
      </View>
    </View> : null : <>
      {kind.reason ? <AppText className="text-[15px] text-foreground-secondary">{maskSecrets(kind.reason)}</AppText> : null}
      {kind.type === "command" ? <Code text={maskSecrets(kind.command)} caption={kind.cwd ?? undefined} />
        : kind.type === "tool" ? <><AppText className="font-app-medium text-[15px]">{kind.name}</AppText><Code text={JSON.stringify(maskInput(kind.input), null, 2)} /></>
        : <View className="gap-1.5">
          <AppText className="text-[15px] text-foreground-secondary">{t("Allow this file change?")}</AppText>
          {kind.changes.map((change, index) => <AppText key={`${change.path}:${index}`} className="font-mono text-[13px]">{change.kind}: {change.path}{change.movePath ? ` → ${change.movePath}` : ""}</AppText>)}
        </View>}
      <AppText className="text-[13px] text-foreground-muted">{t("Applies only to this request. Review the action before allowing it.")}</AppText>
      <View className="flex-row flex-wrap gap-2">
        <Button primary label={t("Allow once")} busy={busy === "accept"} disabled={!!busy} onPress={() => void send("accept", { type: "approval", decision: "accept" })} />
        <Button label={t("Decline")} busy={busy === "decline"} disabled={!!busy} onPress={() => void send("decline", { type: "approval", decision: "decline" })} />
        <Button plain label={t("Cancel turn")} busy={busy === "cancel"} disabled={!!busy} onPress={() => void send("cancel", { type: "approval", decision: "cancel" })} />
      </View>
    </>}
  </View>;
}

function Code(props: { readonly text: string; readonly caption?: string }) {
  return <View className="gap-1">
    <ScrollView className="max-h-40 rounded-xl bg-screen" contentContainerClassName="p-2.5" nestedScrollEnabled>
      <AppText selectable className="font-mono text-[12px] text-foreground-secondary">{props.text}</AppText>
    </ScrollView>
    {props.caption ? <AppText numberOfLines={1} ellipsizeMode="middle" className="text-[12px] text-foreground-muted">{props.caption}</AppText> : null}
  </View>;
}

function Button(props: { readonly label: string; readonly onPress: () => void; readonly primary?: boolean; readonly plain?: boolean; readonly busy?: boolean; readonly disabled?: boolean }) {
  const tone = props.primary ? "bg-foreground" : props.plain ? "" : "border border-border-subtle";
  return <Pressable onPress={props.onPress} disabled={props.disabled} accessibilityRole="button"
    className={`h-10 flex-row items-center justify-center gap-2 rounded-full px-4 active:opacity-70 disabled:opacity-40 ${tone}`}>
    {props.busy ? <ActivityIndicator size="small" /> : null}
    <AppText className={`font-app-medium text-[15px] ${props.primary ? "text-screen" : ""}`}>{props.label}</AppText>
  </Pressable>;
}

/**
 * Messages waiting for the reply to end, as the desktop's queue: each can steer the running
 * reply now (Claude and Codex) or be cancelled.
 */
export function PromptQueue(props: { readonly computerId: string; readonly sessionId: string; readonly agent: string; readonly items: QueuedPrompt[] }) {
  const steerable = props.agent === "claude" || props.agent === "codex";
  const act = (item: QueuedPrompt, steer: boolean) => void useSirus.getState().unqueue(props.computerId, props.sessionId, item.id, steer)
    .catch((reason: unknown) => Alert.alert("Não foi possível instruir agora", errorText(reason)));
  return <View className="mb-2 overflow-hidden rounded-2xl border border-border-subtle bg-card">
    <View className="flex-row items-center gap-1.5 px-3.5 pb-1 pt-2.5">
      <SymbolView name={"list.number" as AppSymbolName} size={12} tintColorClassName="accent-foreground-muted" />
      <AppText className="text-[13px] text-foreground-muted">{props.items.length === 1 ? "1 pedido na fila" : `${props.items.length} pedidos na fila`} · {t("queue.waiting")}</AppText>
    </View>
    {props.items.map((item, index) => <View key={item.id} className="gap-1 px-3.5 py-2">
      <View className="flex-row items-start gap-2">
        <AppText className="mt-2 text-[13px] text-foreground-muted">{index + 1}</AppText>
        <View className="min-w-0 flex-1 items-end">
          <UserBubble text={item.prompt.trim() ? item.prompt : "Anexos"} computerId={props.computerId} />
        </View>
      </View>
      <View className="flex-row items-center justify-end gap-1">
        {item.failed ? <AppText numberOfLines={2} className="flex-1 text-[12px] text-danger-foreground">{item.failed}</AppText> : null}
        {steerable && item.prompt && !item.attachmentIds.length ? <Pressable onPress={() => act(item, true)} hitSlop={6} className="px-1.5 py-1 active:opacity-60">
          <AppText className="text-[13px] font-app-medium">Instruir agora</AppText>
        </Pressable> : null}
        <Pressable onPress={() => act(item, false)} hitSlop={6} accessibilityLabel="Cancelar" className="size-7 items-center justify-center rounded-full active:bg-row-hover">
          <SymbolView name="xmark" size={10} weight="semibold" tintColorClassName="accent-foreground-muted" />
        </Pressable>
      </View>
    </View>)}
  </View>;
}
