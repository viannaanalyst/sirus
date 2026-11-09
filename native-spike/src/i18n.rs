//! UI language. The source text of the app is Brazilian Portuguese; `t` returns it as is when the
//! locale is Portuguese and looks the English up in `ENTRIES` otherwise. A string with no entry
//! stays in Portuguese. The locale is the host setting `locale` ("pt-BR" or "en"), applied with
//! `set_locale` when the settings load and each time they are saved.
//!
//! Wrap what the user reads, never what the code compares: page names, setting keys and ids keep
//! their Portuguese spelling as the internal id and are translated where they are drawn.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Applies the `locale` setting: "en" (or "en-US") is English; "pt-BR", an empty or unknown value
/// is Portuguese; "system" follows the language of the environment.
pub fn set_locale(code: &str) {
    let english = match code {
        "system" => ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty())
            .is_some_and(|value| value.starts_with("en")),
        other => other.starts_with("en"),
    };
    ENGLISH.store(english, Ordering::Relaxed);
}

/// True while the interface is in English.
pub fn is_en() -> bool {
    FORCED
        .with(std::cell::Cell::get)
        .unwrap_or_else(|| ENGLISH.load(Ordering::Relaxed))
}

thread_local! {
    /// A language forced on this thread only, for `with_locale`.
    static FORCED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Runs `build` with `t` answering in English (or Portuguese) on this thread, whatever the app's
/// language is: the settings search indexes both languages from the pages' own builders.
pub fn with_locale<R>(english: bool, build: impl FnOnce() -> R) -> R {
    let before = FORCED.with(|forced| forced.replace(Some(english)));
    let out = build();
    FORCED.with(|forced| forced.set(before));
    out
}

/// The locale code the host takes for dictation: "en" or "pt-BR".
pub fn code() -> &'static str {
    if is_en() { "en" } else { "pt-BR" }
}

fn table() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| ENTRIES.iter().copied().collect())
}

/// The text in the app's language: `portuguese` itself, or its English when there is one.
pub fn t(portuguese: &'static str) -> &'static str {
    if !is_en() {
        return portuguese;
    }
    table().get(portuguese).copied().unwrap_or(portuguese)
}

/// A whole message from the host, which speaks English, in the app's language: its Portuguese when
/// `ENTRIES` has that sentence, otherwise the message as it came.
pub fn from_host(message: &str) -> String {
    static REVERSE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    if is_en() {
        return message.to_owned();
    }
    let reverse = REVERSE.get_or_init(|| {
        let mut reverse = HashMap::new();
        for (portuguese, english) in ENTRIES {
            reverse.entry(*english).or_insert(*portuguese);
        }
        reverse
    });
    reverse.get(message).copied().unwrap_or(message).to_owned()
}

/// A translated template with its values filled in. Each `{}` takes the next value; `{0}`, `{1}`
/// take the one at that position, so English can reorder them.
pub fn tf(template: &'static str, values: &[&dyn Display]) -> String {
    let template = t(template);
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let name = &after[..close];
        let index = if name.is_empty() {
            next += 1;
            Some(next - 1)
        } else {
            name.parse::<usize>().ok()
        };
        match index.and_then(|i| values.get(i)) {
            Some(value) => out.push_str(&value.to_string()),
            None => out.push_str(&rest[open..open + close + 2]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Portuguese (the source text) and its English, sorted by the Portuguese; keep that order when
/// adding.
const ENTRIES: &[(&str, &str)] = &[
    ("\n… diff truncado.", "\n… diff truncated."),
    (" Liberam {}.", " Frees {}."),
    (" · alertas ligados", " · alerts on"),
    (
        "\"{}\" está trabalhando nesta pasta agora. Espere terminar para não sobrescrever as edições dela.",
        "\"{}\" is working in this folder right now. Wait for it to finish so its edits are not overwritten.",
    ),
    (
        "\"{}\" para de trabalhar agora. Os processos dela são encerrados.",
        "\"{}\" stops working now. Its processes are ended.",
    ),
    (
        "\"{}\" sai da lista. A pasta e os arquivos continuam no disco.",
        "\"{}\" leaves the list. Its folder and files stay on disk.",
    ),
    ("\"{}\" sai do rascunho.", "\"{}\" leaves the draft."),
    (
        "\"{}\" será apagada para sempre.",
        "\"{}\" will be gone for good.",
    ),
    (
        "\"{}\" tem alterações não salvas. Fechar a aba descarta essas alterações.",
        "\"{}\" has unsaved changes. Closing the tab discards them.",
    ),
    (
        "\"{}\" tem mudanças não mantidas em {} nesta pasta. Mantenha ou desfaça nessa sessão primeiro.",
        "\"{}\" has unkept changes to {} in this folder. Keep or undo them in that session first.",
    ),
    ("%d/%m/%Y", "%m/%d/%Y"),
    ("%d/%m/%Y %H:%M", "%m/%d/%Y %H:%M"),
    ("(vazio)", "(empty)"),
    ("+{} aviso", "+{} notice"),
    ("+{} avisos", "+{} notices"),
    ("1 agente", "1 agent"),
    ("1 anexo", "1 attachment"),
    ("1 arquivo", "1 file"),
    ("1 arquivo alterado", "1 changed file"),
    ("1 falhou", "1 failed"),
    ("1 passo", "1 step"),
    ("1 pedido na fila", "1 queued request"),
    ("1 repositório indisponível.", "1 repository unavailable."),
    ("1 subagente", "1 subagent"),
    ("5 horas · modelo reserva", "5-hour · reserve model"),
    (
        "60 fps fica mais suave e gera arquivos maiores. A página é capturada o mais rápido que ela permitir.",
        "60 fps is smoother and makes larger files. The page is captured as fast as it allows.",
    ),
    (
        "A CLI do GitHub (gh) não está instalada.",
        "GitHub CLI (gh) is not installed.",
    ),
    (
        "A CLI do GitHub (gh) não está instalada. Instale-a e execute `gh auth login`.",
        "GitHub CLI (gh) is not installed. Install it and run `gh auth login`.",
    ),
    (
        "A CLI do GitHub não está conectada. Execute `gh auth login` em um terminal.",
        "GitHub CLI is not signed in. Run `gh auth login` in a terminal.",
    ),
    (
        "A CLI do GitHub não foi iniciada.",
        "GitHub CLI did not start.",
    ),
    (
        "A CLI do GitHub não forneceu um código de login. Tente novamente.",
        "GitHub CLI did not give a sign-in code. Try again.",
    ),
    (
        "A CLI do provedor não está instalada.",
        "Provider CLI is not installed.",
    ),
    ("A URL não é permitida", "URL is not allowed"),
    (
        "A barra inferior mostra o consumo de até dois provedores escolhidos aqui. Sem nenhum, ela mostra o provedor da conversa aberta.",
        "The bottom bar shows the usage of up to two providers chosen here. With none, it shows the open conversation's provider.",
    ),
    (
        "A busca atingiu seu limite. Algumas skills podem não aparecer.",
        "The search reached its limit. Some skills may not appear.",
    ),
    (
        "A cada 5 minutos, avança o checkout do projeto quando ele está no branch padrão do origin, sem mudanças locais e sem conversa rodando nele. Nunca faz merge, rebase nem mexe numa árvore com mudanças.",
        "Every 5 minutes, fast-forwards the project checkout when it is on origin's default branch with no local changes and no conversation running in it. It never merges, rebases or touches a tree with changes.",
    ),
    (
        "A captura do anexo foi substituída.",
        "Attachment snapshot was replaced.",
    ),
    (
        "A compactação automática aceita um número de 100000 a 1000000, ou fica vazia.",
        "Auto-compact takes a number from 100000 to 1000000, or stays empty.",
    ),
    (
        "A conexão com o computador caiu.",
        "The connection to the computer dropped.",
    ),
    (
        "A conexão com o motor caiu: {}",
        "The connection to the engine dropped: {}",
    ),
    (
        "A conta da CLI mudou; atualize o uso novamente.",
        "CLI account changed; refresh usage again.",
    ),
    (
        "A conta padrão não pode ser removida aqui.",
        "The default account cannot be removed here.",
    ),
    (
        "A conta padrão não pode ser renomeada aqui.",
        "The default account cannot be renamed here.",
    ),
    (
        "A conversa ainda está em andamento.",
        "The conversation is still working.",
    ),
    ("A conversa continua.", "The conversation continues."),
    (
        "A conversa está sendo compactada. Envie a mensagem quando terminar.",
        "The conversation is being compacted. Send the message after it finishes.",
    ),
    (
        "A conversa mudou enquanto era movida; tente de novo.",
        "The conversation changed while it was moving; try again.",
    ),
    (
        "A conversa principal deste chat lateral foi removida.",
        "The main session of this side chat was removed.",
    ),
    (
        "A conversa é grande demais para exportar.",
        "The conversation is too large to export.",
    ),
    (
        "A correção exige um conjunto preparado completo e sem conflitos neste espaço de trabalho.",
        "Amend requires a complete, conflict-free staged set inside this workspace",
    ),
    (
        "A criação de espaço de trabalho não está disponível nesta plataforma.",
        "Workspace creation is unavailable on this platform.",
    ),
    (
        "A descrição é longa demais.",
        "The description is too long.",
    ),
    (
        "A fila aceita até 8 pedidos de 64 KiB cada. Confira também se a troca de modelo terminou.",
        "The queue holds up to 8 requests of 64 KiB each. Also check that any model change has finished.",
    ),
    (
        "A fila de respostas nativas está indisponível",
        "Native response queue is unavailable",
    ),
    (
        "A fila pausou após uma falha. Confira a conversa antes de continuar.",
        "The queue paused after a failure. Check the conversation before continuing.",
    ),
    (
        "A fila pausou porque você parou o agente. Continue quando quiser.",
        "The queue paused because you stopped the agent. Continue when you want.",
    ),
    (
        "A fonte do sistema está ativa; a escolha fica guardada para depois.",
        "The system font is on; your choice is kept for later.",
    ),
    (
        "A galeria é feita de componentes React da janela web; esta janela nativa não a renderiza.",
        "The gallery is made of React components from the web window; this native window does not render it.",
    ),
    (
        "A geração do processo nativo não existe mais",
        "Native process generation no longer exists",
    ),
    ("A gravação é longa demais.", "The recording is too long."),
    (
        "A imagem deve ter no máximo 10 MiB.",
        "The image must be at most 10 MiB.",
    ),
    (
        "A imagem deve ter no máximo 30 MiB.",
        "The image must be at most 30 MiB.",
    ),
    ("A imagem está vazia.", "The image is empty."),
    (
        "A imagem não está mais neste Mac.",
        "The image is no longer on this Mac.",
    ),
    (
        "A imagem é maior que 25 MB.",
        "The image is larger than 25 MB.",
    ),
    ("A instalação falhou: {}", "Install failed: {}"),
    (
        "A instrução e os arquivos são longos demais.",
        "The instruction and its files are too long.",
    ),
    (
        "A lista está incompleta: há mais arquivos do que o limite.",
        "The list is incomplete: there are more files than the limit.",
    ),
    (
        "A mesclagem não está disponível agora.",
        "Merging is not available right now.",
    ),
    (
        "A mesclagem usa \"{}\" e entra em {}. Isso muda o repositório no GitHub.",
        "The merge uses \"{}\" and lands in {}. This changes the repository on GitHub.",
    ),
    (
        "A operação do Git excedeu os limites de tempo ou de saída, ou não pôde ser executada. Atualize antes de tentar de novo.",
        "Git operation exceeded its time/output limits or could not run. Refresh before retrying.",
    ),
    (
        "A operação nativa não pôde ser concluída.",
        "Native operation could not complete.",
    ),
    (
        "A pasta desta conversa está ausente e não pode ser restaurada.",
        "This session's folder is missing and cannot be restored.",
    ),
    (
        "A pergunta exige uma resposta não vazia de no máximo 8 KiB",
        "Question requires one non-empty answer of at most 8 KiB",
    ),
    (
        "A permissão de reconhecimento de voz expirou.",
        "Speech recognition permission timed out.",
    ),
    (
        "A permissão de reconhecimento de voz foi negada. Ative-a em Ajustes do Sistema → Privacidade e Segurança → Reconhecimento de Voz.",
        "Speech recognition permission was denied. Enable it in System Settings → Privacy & Security → Speech Recognition.",
    ),
    (
        "A permissão do microfone expirou.",
        "Microphone permission timed out.",
    ),
    (
        "A permissão do microfone foi negada. Ative-a em Ajustes do Sistema → Privacidade e Segurança → Microfone.",
        "Microphone permission was denied. Enable it in System Settings → Privacy & Security → Microphone.",
    ),
    (
        "A personalização do Dock não está disponível neste host.",
        "Dock customization is not available on this host.",
    ),
    (
        "A preparação do Git mudou. Atualize antes de continuar.",
        "Git preparation changed. Refresh before continuing.",
    ),
    (
        "A página de login do serviço não é segura.",
        "The service's sign-in page is not secure.",
    ),
    (
        "A página de login não é um endereço https.",
        "The sign-in page is not an https address.",
    ),
    ("A página diz", "The page says"),
    ("A página pede confirmação", "The page asks you to confirm"),
    ("A página pede um valor", "The page asks for a value"),
    ("A raiz do Git está indisponível", "Git root is unavailable"),
    (
        "A responsabilidade pelo título do commit mudou",
        "Commit title ownership changed",
    ),
    (
        "A resposta do registro de provedores excede o limite seguro.",
        "Provider registry response exceeds the safe limit.",
    ),
    (
        "A resposta do registro de provedores não é um JSON válido.",
        "Provider registry response is not valid JSON.",
    ),
    (
        "A resposta nativa expirou; interrompa este turno antes de tentar de novo",
        "Native response timed out; stop this turn before retrying",
    ),
    (
        "A resposta não corresponde à solicitação nativa pendente",
        "Response does not match the native pending request",
    ),
    (
        "A resposta não é uma opção nativa oferecida",
        "Answer is not a native offered option",
    ),
    (
        "A resposta terminou antes de a mensagem entrar na fila.",
        "The reply ended before the message was queued.",
    ),
    (
        "A resposta terminou antes de interromper a tarefa.",
        "The reply ended before stopping the task.",
    ),
    (
        "A resposta terminou antes de mover o subagente.",
        "The reply ended before moving the subagent.",
    ),
    (
        "A resposta terminou antes de receber a instrução.",
        "The reply ended before taking the instruction.",
    ),
    (
        "A saída do login excedeu o limite seguro.",
        "Sign-in output exceeded its safe limit.",
    ),
    (
        "A senha é removida do Chaveiro. Isso não pode ser desfeito.",
        "The password is removed from the Keychain. This cannot be undone.",
    ),
    (
        "A solicitação de título de commit não pertence a esta conversa",
        "Commit title request does not belong to this session",
    ),
    (
        "A solicitação do Codex já foi respondida ou não está mais pendente",
        "Codex request already answered or no longer pending",
    ),
    (
        "A solicitação está desatualizada, não pertence a esta conversa ou já foi respondida",
        "Request is stale, foreign or already answered",
    ),
    (
        "A tarefa da equipe usa um modelo indisponível.",
        "Team task uses a model that is unavailable.",
    ),
    (
        "A tarefa da equipe usa um provedor indisponível.",
        "Team task uses a provider that is unavailable.",
    ),
    ("A tela não conectou: {}", "The screen did not connect: {}"),
    (
        "A thread do Codex não corresponde a esta conversa, projeto, espaço de trabalho ou modelo",
        "Codex thread does not match this session, project, workspace or model",
    ),
    (
        "A versão nova é baixada e instalada e espera você reiniciar. Se você não reiniciar, ela entra na próxima vez que abrir o app.",
        "A new version is downloaded and installed, then waits for you to restart. If you don't, it starts the next time you open the app.",
    ),
    ("A versão {} está disponível.", "Version {} is available."),
    (
        "A versão {} está instalada. Ela começa quando você reiniciar ou abrir o app de novo.",
        "Version {} is installed. It starts when you restart or open the app again.",
    ),
    ("ALTERAÇÕES", "CHANGES"),
    ("Abaixo", "Below"),
    ("Abas de conversa", "Conversation tabs"),
    ("Aberta", "Open"),
    ("Aberto", "Open"),
    ("Abertos", "Open"),
    ("Abra os arquivos do projeto.", "Open the project files."),
    ("Abra um app ou qualquer site.", "Open an app or any site."),
    ("Abra um projeto", "Open a project"),
    (
        "Abra {} e digite o código. Esta tela termina quando o GitHub confirmar.",
        "Open {} and enter the code. This screen finishes when GitHub confirms.",
    ),
    (
        "Abre o painel Ambiente automaticamente. Quando desativado, ele fica fechado até você abrir. A última ação de abrir/fechar também atualiza esta preferência.",
        "Open the chat Environment panel automatically on normal threads. When off, the panel stays closed until you open it. Your last open/close also updates this preference.",
    ),
    ("Abrindo documento…", "Opening document…"),
    ("Abrindo o 1Password…", "Opening 1Password…"),
    ("Abrindo o chat lateral…", "Opening the side chat…"),
    ("Abrir", "Open"),
    ("Abrir Ajustes do Sistema", "Open System Settings"),
    ("Abrir Alterações", "Open Changes"),
    ("Abrir Arquivos", "Open Files"),
    ("Abrir Chat lateral", "Open Side chat"),
    ("Abrir MR", "Show MR"),
    ("Abrir Navegador", "Open Browser"),
    ("Abrir PR", "Open PR"),
    ("Abrir Pull Requests", "Open Pull Requests"),
    ("Abrir Simulador", "Open Simulator"),
    ("Abrir Terminal", "Open Terminal"),
    ("Abrir Uso", "Open Usage"),
    (
        "Abrir a prévia quando responder",
        "Open the preview when it answers",
    ),
    ("Abrir ao lado", "Open to the side"),
    ("Abrir como rascunho", "Open as draft"),
    (
        "Abrir diretório de dados do Sirus",
        "Open Sirus data directory",
    ),
    ("Abrir em abas", "Open in tabs"),
    ("Abrir links no", "Open links in"),
    ("Abrir no GitHub", "Open on GitHub"),
    ("Abrir no editor", "Open in the editor"),
    ("Abrir no navegador padrão", "Open in default browser"),
    ("Abrir ou fechar a sidebar", "Open or close the sidebar"),
    ("Abrir ou fechar o chat lateral", "Toggle side chat"),
    (
        "Abrir ou fechar o painel lateral",
        "Open or close the side panel",
    ),
    ("Abrir pasta de logs", "Open logs folder"),
    ("Abrir por padrão", "Open by default"),
    ("Abrir projeto", "Open Project"),
    ("Abrir prévia", "Open preview"),
    (
        "Abrir pull requests e issues",
        "Open pull requests and issues",
    ),
    ("Abrir terminal", "Open Terminal"),
    ("Abrir um painel", "Open a panel"),
    ("Abriu", "Opened"),
    ("Acessibilidade", "Accessibility"),
    ("Acesso completo", "Full access"),
    ("Acesso dos agentes ao navegador", "Agent browser access"),
    ("Acesso remoto", "Remote access"),
    ("Acesso total", "Full access"),
    ("Acima", "Above"),
    ("Acumulado", "Cumulative"),
    (
        "Adiciona as ferramentas de computador aos próximos turnos. Cada app ainda precisa da sua aprovação uma vez por sessão.",
        "Adds computer tools to new turns. Each app still needs your approval once per session.",
    ),
    ("Adicionado", "Added"),
    ("Adicionado em {}", "Added {}"),
    (
        "Adicionado em {} · último uso {}",
        "Added {} · last used {}",
    ),
    ("Adicionar", "Add"),
    ("Adicionar ao chat", "Add to chat"),
    ("Adicionar ação", "Add action"),
    ("Adicionar conector personalizado", "Add a custom connector"),
    ("Adicionar conta", "Add account"),
    ("Adicionar login", "Add login"),
    ("Adicionar perfil", "Add profile"),
    ("Adicionar personalizado", "Add custom"),
    ("Adicionar token", "Add token"),
    ("Adicionar um computador", "Add a computer"),
    (
        "Adicione pastas com SKILL.md aqui. Skills invocadas funcionam entre provedores; a cópia do próprio provedor tem prioridade.",
        "Add folders with SKILL.md here. Invoked skills work across providers; the provider's own copy takes priority.",
    ),
    (
        "Adicione um projeto para abrir links de prompt.",
        "Add a project to open prompt links.",
    ),
    (
        "Adicione um token do Bitbucket em Configurações → Git para ver e criar pull requests.",
        "Add a Bitbucket token in Settings → Git to see and open pull requests.",
    ),
    (
        "Adicione uma pasta com SKILL.md no diretório acima ou na pasta de skills de um provedor e atualize.",
        "Add a folder with SKILL.md to the directory above or to a provider's skills folder, then refresh.",
    ),
    ("Age sem perguntar.", "Acts without asking."),
    ("Agente", "Agent"),
    ("Agentes", "Agents"),
    ("Agentes podem usar este login", "Agents may use this login"),
    ("Agora não", "Not now"),
    ("Aguardando o navegador…", "Waiting for the browser…"),
    ("Aguardando resposta", "Waiting for an answer"),
    (
        "Aguarde a resposta terminar e envie /compact.",
        "Wait for the reply to finish, then send /compact.",
    ),
    (
        "Aguarde o agente terminar antes de desfazer alterações.",
        "Wait for the agent to finish before undoing changes.",
    ),
    (
        "Aguarde o agente terminar antes de executar um script do projeto.",
        "Wait for the agent to finish before running a project script.",
    ),
    (
        "Aguarde todas as tarefas terminarem antes de mesclar.",
        "Wait until every task finishes before merging.",
    ),
    (
        "Aguarde todas as tarefas terminarem antes de resolver.",
        "Wait until every task finishes before resolving.",
    ),
    (
        "Ainda não há o que compactar: esta conversa não começou.",
        "There is nothing to compact yet: this conversation has not started.",
    ),
    (
        "Ajusta o espaçamento sem mudar o tamanho do texto.",
        "Adjusts spacing without changing the text size.",
    ),
    (
        "Alertas de pull request inválidos.",
        "Invalid pull request alerts.",
    ),
    ("Alguém", "Someone"),
    ("Alterar arquivos", "Change files"),
    ("Alterações", "Changes"),
    ("Alterações desfeitas", "Changes undone"),
    ("Alterações em arquivos", "File changes"),
    ("Alternar nível de raciocínio", "Cycle reasoning effort"),
    ("Alternar painel direito", "Toggle right panel"),
    ("Alto", "High"),
    ("Alto contraste", "High contrast"),
    ("Ambiente", "Environment"),
    ("Ameixa", "Plum"),
    ("Amostra", "Sample"),
    ("Analisar anexos", "Analyze attachments"),
    (
        "Analisar e propor antes de editar",
        "Analyze and propose before editing",
    ),
    (
        "Analytics de produto, flags e erros",
        "Product analytics, flags and errors",
    ),
    ("Anexar janela", "Attach window"),
    ("Anexo de imagem inválido.", "Invalid image attachment."),
    ("Anexo duplicado.", "Duplicate attachment."),
    ("Anexo indisponível.", "Attachment is unavailable."),
    (
        "Anexo indisponível. Anexe-o novamente.",
        "Attachment is unavailable. Attach it again.",
    ),
    ("Animações da interface", "Interface animations"),
    ("Anotação anexada", "Annotation attached"),
    (
        "Ao abrir o Sirus e a cada poucas horas.",
        "When Sirus opens and every few hours.",
    ),
    ("Ao terminar", "On finish"),
    (
        "Aparece nas abas e no composer",
        "Shown in the tabs and the composer",
    ),
    ("Aparelhos", "Devices"),
    ("Aparência", "Appearance"),
    (
        "Aperte Esc ou Parar no indicador flutuante para revogar todos os apps de uma vez.",
        "Press Escape or Stop on the floating indicator to revoke every app at once.",
    ),
    (
        "Aperte esc de novo para parar o agente",
        "Press esc again to stop the agent",
    ),
    (
        "Aperte ⌘Q de novo para fechar o Sirus",
        "Press ⌘Q again to close Sirus",
    ),
    (
        "Aplicada ao enviar e mantida nos seus próximos envios manuais.",
        "Applied when you send, then retained for your next manual turns.",
    ),
    ("Aplicativo", "App"),
    ("Aprova o que for seguro.", "Approves what is safe."),
    ("Aprovar", "Approve"),
    ("Aprovar por mim", "Auto-review"),
    ("Aprovação por app", "Approve each app"),
    ("Aprovou", "Approved"),
    ("Areia", "Sand"),
    (
        "Armazenamento da conta indisponível",
        "Account storage unavailable",
    ),
    (
        "Arquivo binário: não dá para mostrar o conteúdo aqui.",
        "Binary file: its content cannot be shown here.",
    ),
    (
        "Arquivo binário: prévia textual indisponível.",
        "Binary file: no textual preview available.",
    ),
    (
        "Arquivo grande demais para editar: somente leitura.",
        "File too large to edit: read-only.",
    ),
    ("Arquivo vazio.", "Empty file."),
    ("Arquivos", "Files"),
    ("Arquivos ({})", "Files ({})"),
    ("Arquivos e pastas", "Files and folders"),
    ("Arquivos que impedem o pull:", "Files blocking the pull:"),
    ("Arquivos recentes", "Recent files"),
    (
        "Arraste para escolher a sua cor.",
        "Drag to pick your color.",
    ),
    (
        "As alterações ainda contêm marcadores de conflito.",
        "The changes still contain conflict markers.",
    ),
    (
        "As alterações do agente auxiliar não puderam ser salvas, então o worktree dele foi mantido.",
        "A helper's changes could not be saved, so its worktree was kept.",
    ),
    (
        "As alterações do agente excedem o limite de mesclagem de 8 MiB.",
        "The worker's changes exceed the 8 MiB merge limit.",
    ),
    (
        "As alterações não se aplicam de forma limpa.",
        "The changes do not apply cleanly.",
    ),
    (
        "As alterações preparadas mudaram ou não são mais elegíveis para gerar o título do commit",
        "Prepared changes changed or are no longer eligible for commit title generation",
    ),
    (
        "As alterações que ainda não viraram commit serão perdidas. Não dá para desfazer.",
        "Changes that are not in a commit yet will be lost. This cannot be undone.",
    ),
    (
        "As combinações de atalho devem ser únicas.",
        "Shortcut combinations must be unique.",
    ),
    (
        "As dimensões da imagem excedem o limite seguro.",
        "Image dimensions exceed the safe limit.",
    ),
    (
        "As gravações rodam a 30 ou 60 quadros por segundo.",
        "Recordings run at 30 or 60 frames per second.",
    ),
    (
        "As instruções devem ter de 1 a 65.536 bytes de texto.",
        "Instructions must have 1 to 65,536 bytes of text.",
    ),
    (
        "As leituras do GitHub foram pausadas após o limite de requisições",
        "GitHub reads are paused after a rate limit",
    ),
    (
        "As mensagens desta conversa não podem ser editadas.",
        "This conversation's messages cannot be edited.",
    ),
    (
        "As notas de atualização não são válidas.",
        "Update notes are not valid.",
    ),
    (
        "As notas excedem o limite de 16.384 caracteres",
        "Notes exceed the 16,384 character limit",
    ),
    (
        "As opções desta página voltam ao padrão do app.",
        "This page's options go back to the app's defaults.",
    ),
    (
        "As regras do repositório impedem a mesclagem.",
        "The repository rules block the merge.",
    ),
    (
        "As respostas devem ter de 1 a 65.536 bytes de texto.",
        "Replies must have 1 to 65,536 bytes of text.",
    ),
    (
        "As skills invocadas excedem o limite de 64 KiB por turno.",
        "Invoked skills exceed the 64 KiB turn limit.",
    ),
    (
        "As tarefas da equipe só podem ser editadas ou removidas.",
        "Team tasks can only be edited or removed.",
    ),
    ("Atalho", "Shortcut"),
    ("Atalho global de captura", "Global snap shortcut"),
    ("Atalhos", "Keybindings"),
    ("Ativa", "Active"),
    ("Ativar modo debug", "Turn debug mode on"),
    (
        "Ative este modelo em Configurações antes de transferir.",
        "Enable this model in Settings before handing off.",
    ),
    (
        "Ative este provedor e modelo em Configurações antes de bifurcar.",
        "Enable this provider and model in Settings before forking.",
    ),
    (
        "Ative este provedor em Configurações antes de abrir um chat lateral.",
        "Enable this provider in Settings before opening a side chat.",
    ),
    (
        "Ative este provedor em Configurações antes de transferir.",
        "Enable this provider in Settings before handing off.",
    ),
    (
        "Ative o acesso remoto primeiro.",
        "Turn on remote access first.",
    ),
    ("Atividade", "Activity"),
    ("Atividade recente", "Recent activity"),
    ("Ativos {}", "Active {}"),
    ("Atribuídos", "Assignees"),
    ("Atualizado a partir de {}", "Updated from {}"),
    ("Atualizado agora", "Updated now"),
    ("Atualizado há {} h", "Updated {} h ago"),
    ("Atualizado há {} min", "Updated {} min ago"),
    ("Atualizando…", "Updating…"),
    ("Atualizar", "Update"),
    (
        "Atualizar a senha salva de {}?",
        "Update the saved password for {}?",
    ),
    (
        "Atualizar o branch padrão automaticamente",
        "Automatically pull the default branch",
    ),
    (
        "Atualizar referências ao abrir projeto",
        "Automatically fetch when opening project",
    ),
    ("Atualizar skills", "Refresh skills"),
    ("Atualização pronta · Reiniciar", "Update ready · Restart"),
    ("Atualizações", "Updates"),
    (
        "Atualizações do provedor exigem confirmação.",
        "Provider updates require confirmation.",
    ),
    (
        "Atualize o catálogo de modelos antes de alterar as opções de execução.",
        "Refresh the model catalog before changing execution options.",
    ),
    (
        "Aumenta ou diminui tudo.",
        "Makes everything larger or smaller.",
    ),
    ("Aumentar zoom", "Zoom in"),
    ("Auto-compactar depois de", "Auto-compact after"),
    ("Autocorreção", "Autocorrect"),
    ("Automático", "Automatic"),
    ("Avançado", "Advanced"),
    ("Avançar", "Forward"),
    ("Avançou", "Went forward"),
    ("Avisar com o app em foco", "Notify while app is in focus"),
    ("Avisos de atividade", "Activity alerts"),
    ("Avisos no app", "Activity toasts"),
    ("Azul", "Blue"),
    ("Ações", "Actions"),
    ("Ações recentes", "Recent actions"),
    ("Baixando a versão {}…", "Downloading version {}…"),
    ("Baixando o Sirus {}…", "Downloading Sirus {}…"),
    (
        "Baixar atualizações em segundo plano",
        "Download updates in the background",
    ),
    ("Baixar e instalar", "Download and install"),
    ("Baixo", "Low"),
    ("Bancos Prisma Postgres", "Prisma Postgres databases"),
    ("Barra", "Bar"),
    ("Barra de título", "Title bar"),
    ("Barra de ícones", "Icon rail"),
    (
        "Barra de ícones, com as conversas nas abas, ou uma lista com todas as conversas.",
        "An icon rail, with conversations in the tabs, or a list with every conversation.",
    ),
    ("Barra lateral", "Sidebar"),
    ("Bem-vindo ao Sirus", "Welcome to Sirus"),
    ("Bifurcar", "Fork"),
    ("Bloco", "Block"),
    ("Bloquear", "Lock"),
    ("Boas-vindas", "Welcome"),
    ("Branch do Git inválido", "Invalid Git branch"),
    ("Branch não encontrado", "Branch not found"),
    ("Branco", "White"),
    ("Brasa", "Ember"),
    ("Busca do Finder", "Finder search"),
    ("Busca na web", "Web search"),
    (
        "Busca o branch padrão do origin e baseia novos worktrees nele, e não no branch local. Sem origin ou sem rede, eles começam do branch local.",
        "Fetches origin's default branch and bases new worktrees on it instead of the local branch. Without origin or a network, they start from the local branch.",
    ),
    ("Buscando atualização…", "Looking for an update…"),
    ("Buscando…", "Searching…"),
    ("Buscar", "Search"),
    ("Buscar arquivos", "Search files"),
    (
        "Buscar arquivos pelo nome…  (> para comandos)",
        "Search files by name…  (> for commands)",
    ),
    ("Buscar atualização", "Check for updates"),
    ("Buscar conectores", "Search connectors"),
    ("Buscar configurações", "Search Settings"),
    ("Buscar conta", "Search accounts"),
    (
        "Buscar conversas, arquivos e projetos",
        "Search conversations, files and projects",
    ),
    ("Buscar em todas as conversas", "Search all conversations"),
    ("Buscar modelos...", "Search models..."),
    ("Buscar nesta conversa", "Find in conversation"),
    ("Buscar ou criar worktree…", "Search or create worktree…"),
    (
        "Buscar por título, repositório, autor ou #",
        "Search by title, repository, author or #",
    ),
    ("Buscar projetos…", "Search projects…"),
    ("Buscar skills", "Search skills"),
    (
        "Buscar, ler e editar páginas e bancos",
        "Search, read and edit pages and databases",
    ),
    ("Buscou", "Searched"),
    ("Cache limpo", "Cache cleared"),
    (
        "Cada anexo deve ter no máximo 10 MiB.",
        "Each attachment must be at most 10 MiB.",
    ),
    (
        "Cada aparelho precisa de um QR code deste Mac para entrar.",
        "Each device needs a QR code from this Mac to get in.",
    ),
    (
        "Cada arquivo deve ter até 10 MiB.",
        "Each file must be at most 10 MiB.",
    ),
    (
        "Cada conversa fica na conta que a iniciou.",
        "Each conversation stays on the account that started it.",
    ),
    (
        "Cada pergunta leva o que a sessão principal está fazendo agora. Começa em só leitura e usa o mesmo workspace.",
        "Each question carries what the main session is doing right now. It starts read-only and uses the same workspace.",
    ),
    ("Cada script tem até 8 KiB.", "Each script is up to 8 KiB."),
    (
        "Cada tarefa da equipe precisa de 1 a 8 caminhos.",
        "Each team task needs 1 to 8 paths.",
    ),
    ("Caminho", "Path"),
    (
        "Caminho Git do espaço de trabalho inválido",
        "Invalid workspace Git path",
    ),
    (
        "Caminho do executável do provedor inválido.",
        "Invalid provider executable path.",
    ),
    (
        "Caminho do executável. Vazio usa o detectado.",
        "Executable path. Empty uses the detected one.",
    ),
    ("Cancelar", "Cancel"),
    ("Cancelar geração do título", "Cancel title generation"),
    ("Captura", "Screenshot"),
    ("Captura da tela", "Screenshot"),
    ("Captura da tela inteira", "Full-screen screenshot"),
    (
        "Captura da tela inteira (para a área de transferência)",
        "Full-screen screenshot (to the clipboard)",
    ),
    ("Captura de anexo inválida.", "Invalid attachment snapshot."),
    ("Captura de janela", "Window snap"),
    ("Captura de uma seleção", "Selection screenshot"),
    (
        "Captura de uma seleção (para a área de transferência)",
        "Selection screenshot (to the clipboard)",
    ),
    (
        "Captura e gravação de tela",
        "Screenshot and screen recording",
    ),
    ("Captura salva em {}", "Screenshot saved to {}"),
    ("Capturar a página", "Capture the page"),
    ("Capturar uma janela aberta", "Capture an open app window"),
    (
        "Capturas da janela do app aprovado.",
        "Screenshots of the approved app's window.",
    ),
    ("Carregando catálogo de modelos…", "Loading model catalog…"),
    ("Carregando…", "Loading…"),
    ("Carregar mais", "Load more"),
    ("Carregar mensagens anteriores", "Load earlier messages"),
    ("Carregou ferramentas", "Loaded tools"),
    ("Carvão", "Charcoal"),
    ("Chamada de ferramenta", "Tool call"),
    ("Chat lateral", "Side chat"),
    ("Chave conectada · {}", "Key connected · {}"),
    ("Checkout atual", "Current checkout"),
    ("Ciano", "Cyan"),
    ("Claro", "Light"),
    ("Clicar de novo para liberar", "Click again to free up"),
    ("Clicar de novo para limpar", "Click again to clean up"),
    ("Clicar de novo para remover", "Click again to remove"),
    ("Clicar de novo para restaurar", "Click again to reset"),
    ("Clicar de novo para sair", "Click again to sign out"),
    ("Clicou", "Clicked"),
    (
        "Clientes, pagamentos e assinaturas",
        "Customers, payments and subscriptions",
    ),
    (
        "Clique de novo para voltar ao do tema.",
        "Click again to go back to the theme's.",
    ),
    (
        "Clique para compactar antes de enviar",
        "Click to compact before sending",
    ),
    ("Clique para entrar", "Click to sign in"),
    (
        "Clique para manter o histórico nesta mensagem",
        "Click to keep the history for this message",
    ),
    ("Clássico", "Classic"),
    ("Colar token", "Paste token"),
    (
        "Cole email:token (um API token da Atlassian) ou um access token do repositório. Sem token, o Sirus usa as credenciais do Git para bitbucket.org.",
        "Paste email:token (an Atlassian API token) or a repository access token. Without one, Sirus uses Git's credentials for bitbucket.org.",
    ),
    (
        "Cole o token sem espaços, como email:token ou um access token.",
        "Paste the token without spaces, as email:token or an access token.",
    ),
    (
        "Cole um token do serviço; ele fica no Keychain, nunca nos ajustes.",
        "Paste a token from the service; it is kept in the Keychain, never in settings.",
    ),
    ("Comando", "Command"),
    ("Comandos", "Commands"),
    (
        "Combinação de atalho não suportada.",
        "Unsupported shortcut combination.",
    ),
    ("Comece uma nova página", "Start a new page"),
    ("Comentar", "Comment"),
    ("Comentou", "Commented"),
    ("Comentário enviado.", "Comment sent."),
    ("Começar", "Get started"),
    ("Commit do Git inválido", "Invalid Git commit"),
    ("Commit e Push", "Commit and Push"),
    ("Commit feito: {title}", "Committed: {title}"),
    ("Commit inválido.", "Invalid commit."),
    ("Commit não encontrado.", "Commit not found."),
    ("Commit, push e PR", "Commit, push and PR"),
    (
        "Como a imagem é desenhada atrás das conversas.",
        "How the image is drawn behind the conversations.",
    ),
    (
        "Como as ações do agente devem ser aprovadas?",
        "How should agent actions be approved?",
    ),
    (
        "Como o texto da resposta aparece enquanto o agente escreve.",
        "How reply text appears while the agent writes.",
    ),
    ("Compacta", "Compact"),
    ("Compactando o contexto…", "Compacting context…"),
    ("Compactar", "Compact"),
    (
        "Compactar conversas paradas do Claude antes do cache expirar (beta)",
        "Compact idle Claude conversations before the cache expires (beta)",
    ),
    ("Compactar e enviar", "Compact and send"),
    ("Compartilhadas (.agents)", "Shared (.agents)"),
    ("Componentes UI Arc", "UI Arc components"),
    ("Comportamento", "Behavior"),
    ("Comportamento do agente", "Agent behavior"),
    ("Comportamento do chat", "Chat behavior"),
    ("Composer rápido", "Quick composer"),
    ("Composer rápido (⌥Espaço)", "Quick composer (⌥Space)"),
    ("Composer rápido do Sirus ({})", "Sirus quick composer ({})"),
    (
        "Composer rápido do Sirus (⌥Espaço)",
        "Sirus quick composer (⌥Space)",
    ),
    ("Computador", "Computer"),
    ("Computador pareado.", "Computer paired."),
    ("Computadores", "Computers"),
    (
        "Conclua o login no navegador.",
        "Complete sign-in in your browser.",
    ),
    (
        "Conclua, mescle ou descarte a equipe atual antes de planejar outra.",
        "Finish, merge or discard the current team before planning another.",
    ),
    ("Concluir", "Done"),
    ("Concluída", "Done"),
    ("Concluídas", "Done"),
    ("Conectadas", "Connected"),
    ("Conectado", "Connected"),
    ("Conectado como {}", "Signed in as {}"),
    (
        "Conectado pelo glab em {}.",
        "Signed in through glab on {}.",
    ),
    ("Conectado pelo glab.", "Signed in through glab."),
    ("Conectados", "Connected"),
    ("Conectando a {}…", "Connecting to {}…"),
    ("Conectando à tela…", "Connecting to the screen…"),
    ("Conectando…", "Connecting…"),
    ("Conectar", "Connect"),
    ("Conectar outra conta", "Connect another account"),
    ("Conectar um aparelho", "Connect a device"),
    ("Conectar {}", "Connect {}"),
    (
        "Conecte Supabase, Linear, Notion e outros serviços uma vez; Claude, Codex, OpenCode e os outros agentes ACP passam a usá-los.",
        "Connect Supabase, Linear, Notion and other services once; Claude, Codex, OpenCode and the other ACP agents then use them.",
    ),
    ("Conector não encontrado.", "Connector not found."),
    ("Conectores", "Connectors"),
    ("Conexões", "Connections"),
    ("Configurações", "Settings"),
    ("Configurações do projeto", "Project settings"),
    (
        "Configure o endereço público deste Mac para conectar aparelhos.",
        "Set up this Mac's public address to connect devices.",
    ),
    ("Confirmar", "Confirm"),
    ("Confirmar ao excluir", "Confirm deleting"),
    (
        "Confirmar ao fechar um terminal",
        "Confirm closing a terminal",
    ),
    (
        "Confirmar ações destrutivas do Git",
        "Confirm destructive Git actions",
    ),
    (
        "Confirmar fechamento de sessões em execução",
        "Show confirmation before closing running sessions",
    ),
    ("Confirmar push", "Confirm push"),
    ("Confirmações de segurança", "Safety confirmations"),
    (
        "Confirme primeiro a desfazer das alterações deste turno.",
        "Confirm undoing this turn's changes first.",
    ),
    (
        "Confirme primeiro esta alteração no GitHub.",
        "Confirm this GitHub change first.",
    ),
    ("Confortável", "Comfortable"),
    ("Conjunto da equipe", "Team pool"),
    (
        "Consultar Postgres, gerenciar projetos, funções e storage",
        "Query Postgres, manage projects, functions and storage",
    ),
    ("Consumo na barra inferior", "Usage in the bottom bar"),
    ("Consumo na barra lateral", "Usage in the sidebar"),
    ("Conta", "Account"),
    ("Conta do Terminal", "Terminal account"),
    ("Conta padrão", "Default account"),
    ("Contas do GitHub", "GitHub accounts"),
    ("Contas do {}", "{} accounts"),
    ("Contexto compactado", "Context compacted"),
    (
        "Contexto do título do commit indisponível",
        "Commit title context unavailable",
    ),
    ("Contexto não compactado", "Context not compacted"),
    ("Conteúdo não exibido", "Content not shown"),
    ("Continuar", "Continue"),
    (
        "Continuar conversas interrompidas após uma falha",
        "Continue interrupted conversations after a crash",
    ),
    ("Continuar em {}", "Continue on {}"),
    ("Continuar fila", "Continue queue"),
    ("Continue de onde parou.", "Continue where you left off."),
    (
        "Controle mensagens durante a resposta, ditado, turnos terminados, revisão e confirmações de segurança.",
        "Control follow-ups, dictation, finished turns, review defaults and safety confirmations.",
    ),
    ("Conversa", "Conversation"),
    ("Conversas", "Chats"),
    (
        "Conversas do Ask sobre um pull request ficam neste computador.",
        "Ask conversations about a pull request stay on this computer.",
    ),
    ("Conversas e contatos", "Conversations and contacts"),
    (
        "Conversas novas recebem os conectores daqui. Os conectores de um projeto só chegam às conversas dele. Grok, Antigravity, Droid e Pi ainda não aceitam conectores.",
        "New conversations receive the connectors here. A project's connectors reach only its conversations. Grok, Antigravity, Droid and Pi do not take connectors yet.",
    ),
    ("Conversas recentes", "Recent chats"),
    ("Converter", "Convert"),
    ("Converter em rascunho", "Convert to draft"),
    ("Converter {} em rascunho?", "Convert {} to draft?"),
    (
        "Cookies limpos em todas as abas do navegador",
        "Cookies cleared in every browser tab",
    ),
    ("Copiado", "Copied"),
    ("Copiar", "Copy"),
    ("Copiar ID da conversa", "Copy conversation ID"),
    ("Copiar caminho", "Copy path"),
    ("Copiar endereço", "Copy address"),
    ("Copiar link", "Copy link"),
    ("Copiar link do prompt", "Copy prompt link"),
    ("Cor", "Color"),
    ("Cor de destaque", "Accent color"),
    ("Cores", "Colors"),
    ("Cores do diff", "Diff colors"),
    ("Cores do tema", "Theme colors"),
    ("Corrigir", "Amend"),
    ("Corrigir CI automaticamente", "Fix CI automatically"),
    ("Corrigir o último", "Amend last"),
    ("Criando…", "Creating…"),
    ("Criar", "Create"),
    ("Criar MR", "Open MR"),
    ("Criar PR", "Create PR"),
    ("Criar branch…", "Create branch…"),
    ("Criar merge commit", "Create a merge commit"),
    (
        "Criar um branch exige confirmação.",
        "Creating a branch requires confirmation.",
    ),
    (
        "Criar, publicar e gerenciar sites",
        "Create, deploy and manage sites",
    ),
    ("Criação", "Created"),
    ("Custo", "Cost"),
    ("Custo acumulado", "Cumulative cost"),
    ("Custo por tipo", "Cost by type"),
    ("Código", "Code"),
    ("Código de confirmação", "Confirmation code"),
    ("Código de pareamento", "Pairing code"),
    ("Daltônico", "Color-blind"),
    (
        "Daltônico e alto contraste usam azul e laranja.",
        "Color-blind and high contrast use blue and orange.",
    ),
    ("De 10 a 22 px.", "From 10 to 22 px."),
    ("De 11 a 18 px.", "From 11 to 18 px."),
    ("Defina uma meta a perseguir", "Define a goal to pursue"),
    (
        "Defina uma meta a perseguir nesta conversa.",
        "Set a goal to pursue in this conversation.",
    ),
    (
        "Deixa o fundo da janela aparecer através do app.",
        "Lets the window background show through the app.",
    ),
    (
        "Deixa os agentes clicarem nas janelas do próprio Sirus, inclusive em aprovações. Ligue só se confiar nos agentes que usa.",
        "Lets agents click Sirus's own windows, including approvals. Turn it on only if you trust the agents you use.",
    ),
    ("Deixe do seu jeito", "Make it yours"),
    (
        "Deixe o Codex, o Claude Code e o OpenCode verem e usarem apps do Mac que você aprovar, em segundo plano.",
        "Let Codex, Claude Code and OpenCode see and use Mac apps you approve, in the background.",
    ),
    (
        "Deixe vazio para manter a senha salva",
        "Leave empty to keep the saved password",
    ),
    (
        "Deixe vazio para manter o token salvo.",
        "Leave empty to keep the saved token.",
    ),
    ("Delegada para {}", "Delegated to {}"),
    ("Delegar tarefa", "Delegate task"),
    ("Delegou", "Delegated"),
    ("Densidade da interface", "UI density"),
    ("Depois", "Later"),
    (
        "Depois de permitir nos Ajustes do Sistema, feche e reabra o Sirus se o status não atualizar.",
        "After allowing in System Settings, quit and reopen Sirus if the status does not update.",
    ),
    ("Depois do turno", "After the turn"),
    ("Desafixar", "Unpin"),
    ("Desafixar aba", "Unpin tab"),
    ("Descartada", "Dismissed"),
    ("Descartar", "Discard"),
    ("Descartar alterações em {}?", "Discard changes to {}?"),
    ("Descartar alterações?", "Discard changes?"),
    ("Descartar todas as alterações?", "Discard all changes?"),
    ("Descartar tudo", "Discard all"),
    ("Desconhecido", "Unknown"),
    ("Descrição (opcional)", "Description (optional)"),
    (
        "Desenhe sobre a imagem para destacar uma área.",
        "Draw over the image to highlight an area.",
    ),
    ("Desfazer", "Undo"),
    ("Desfazer alterações", "Undo changes"),
    (
        "Desfazer as mudanças deste turno?",
        "Undo this turn's changes?",
    ),
    ("Desfazer traço", "Undo stroke"),
    ("Designs e materiais da marca", "Designs and brand assets"),
    (
        "Designs, componentes e variáveis",
        "Designs, components and variables",
    ),
    ("Desligada", "Off"),
    ("Desligado", "Off"),
    ("Desligar", "Turn off"),
    ("Desligar salvamento automático", "Turn autosave off"),
    ("Despreparar", "Unstage"),
    ("Despreparar tudo", "Unstage all"),
    (
        "Destino do espaço de trabalho inválido.",
        "Invalid workspace destination.",
    ),
    ("Detectado: {}", "Detected: {}"),
    ("Detectar novamente", "Detect again"),
    ("Diagnóstico", "Diagnostics"),
    ("Diagnóstico das CLIs", "CLI diagnostics"),
    ("Digite ao menos 3 letras.", "Type at least 3 letters."),
    (
        "Digite o valor e aperte Enter",
        "Type the value and press Enter",
    ),
    ("Digite um endereço", "Type an address"),
    (
        "Digite um valor de até 16 KB.",
        "Enter a value up to 16 KB.",
    ),
    ("Digitou", "Typed"),
    ("Diminuir zoom", "Zoom out"),
    ("Diretório automático", "Automatic directory"),
    (
        "Diretório de perfil da conta inseguro.",
        "Unsafe account profile directory.",
    ),
    ("Diretório de trabalho", "Working directory behavior"),
    ("Diretório personalizado", "Custom directory"),
    (
        "Disponível só neste Mac: esta janela está em outro computador.",
        "Only available on this Mac: this window is on another computer.",
    ),
    (
        "Distingue duas contas do mesmo serviço, como “Conta Gabriel” e “Conta Moreira”. Os agentes não veem o rótulo.",
        "Tells two accounts of the same service apart, like “Work” and “Personal”. Agents do not see the label.",
    ),
    (
        "Divergiu: {} local e {} do remoto. Faça merge ou rebase à mão.",
        "Diverged: {} local and {} remote commits. Merge or rebase by hand.",
    ),
    ("Do tema", "From theme"),
    (
        "Documentação atualizada de bibliotecas",
        "Up-to-date library documentation",
    ),
    ("Dê um nome ao dispositivo.", "Give the device a name."),
    (
        "Dê um nome e um comando à ação.",
        "Give the action a name and a command.",
    ),
    ("Economia do cache", "Cache savings"),
    ("Editar", "Edit"),
    ("Editar ação", "Edit action"),
    ("Editou", "Edited"),
    ("Editou 1 arquivo", "Edited 1 file"),
    ("Editou {} arquivos", "Edited {} files"),
    ("Efeito de luz nos botões", "Cursor-reactive button effects"),
    ("Efeito do fundo", "Background effect"),
    ("Efeitos e movimento", "Effects and motion"),
    (
        "Ele cita um provedor que o Sirus não tem.",
        "It names a provider Sirus does not have.",
    ),
    (
        "Ele deixa de poder ser mesclado até ficar pronto de novo.",
        "It cannot be merged until it is ready again.",
    ),
    ("Ele não tem prompt.", "It has no prompt."),
    (
        "Ele poderá ver a janela deste app e clicar e digitar nela durante esta sessão.",
        "It will be able to see this app's window and click and type in it during this session.",
    ),
    ("Ele repete um parâmetro.", "It repeats a parameter."),
    (
        "Ele sai da lista de abertos. Dá para reabrir depois.",
        "It leaves the open list. You can reopen it later.",
    ),
    (
        "Ele tem um parâmetro que o Sirus não conhece.",
        "It has a parameter Sirus does not know.",
    ),
    (
        "Ele volta para a lista de abertos.",
        "It goes back to the open list.",
    ),
    (
        "Em Supabase → Project Settings → General. Deixe vazio para o agente ver todos os projetos da conta.",
        "In Supabase → Project Settings → General. Leave empty to let the agent see every project of the account.",
    ),
    (
        "Em branco usa o nome do conector.",
        "Leave empty to use the connector's name.",
    ),
    ("Em breve", "Coming soon"),
    ("Em espera", "Waiting"),
    (
        "Em qualquer app, o atalho captura a janela da frente e anexa à conversa aberta no Sirus. Nada é enviado.",
        "From any app, the shortcut captures the window in front and attaches it to the conversation open in Sirus. Nothing is sent.",
    ),
    ("Em uso", "In use"),
    ("Embaixo", "Bottom"),
    (
        "Embaixo, no painel lateral ou como abas na barra de título. ⌘J abre um terminal e ⇧⌘J divide à direita.",
        "At the bottom, in the side panel or as tabs in the title bar. ⌘J opens a terminal and ⇧⌘J splits to the right.",
    ),
    (
        "Encontramos estes no seu Mac. Um já basta para começar.",
        "We found these on your Mac. One is enough to get started.",
    ),
    (
        "Encontrar, adicionar e concluir tarefas",
        "Find, add and complete tasks",
    ),
    (
        "Encontrar, criar e atualizar issues e projetos",
        "Find, create and update issues and projects",
    ),
    ("Encontrei {}.", "Found {}."),
    ("Endereço", "Address"),
    ("Endereço da prévia", "Preview address"),
    ("Endereço do QR code", "QR code address"),
    ("Endereço inválido.", "Invalid address."),
    ("Entendi", "Got it"),
    ("Enter durante o ditado", "Enter while dictating"),
    ("Enter envia · Esc fecha", "Enter sends · Esc closes"),
    ("Entrada", "Input"),
    ("Entrada em cache", "Cached input"),
    (
        "Entrada indisponível por enquanto.",
        "Input is not available yet.",
    ),
    (
        "Entrada secreta não é compatível; use a CLI do provedor",
        "Secret input is not supported; use the provider CLI",
    ),
    ("Entrada sem cache", "Uncached input"),
    ("Entrar", "Sign in"),
    ("Entrar de novo", "Sign in again"),
    (
        "Entrar só pelo app Mac: o login abre o navegador.",
        "Sign-in is only available in the Mac app: it opens the browser.",
    ),
    (
        "Entre na conta padrão usando a própria CLI do provedor.",
        "Sign in to the default account using the provider's own CLI.",
    ),
    (
        "Entre no 1Password e ligue \"Integrar com o 1Password CLI\" nos ajustes dele.",
        "Sign in to 1Password and turn on \"Integrate with 1Password CLI\" in its settings.",
    ),
    (
        "Entre no GitHub CLI em Configurações → Git.",
        "Sign in to the GitHub CLI in Settings → Git.",
    ),
    (
        "Entre nos conectores pelo app do Mac; um token funciona aqui.",
        "Sign in to connectors from the Mac app; a token works here.",
    ),
    (
        "Envia este branch e abre o merge request para o branch padrão.",
        "Pushes this branch and opens the merge request into the default branch.",
    ),
    (
        "Envia este branch e abre o pull request para o branch padrão.",
        "Pushes this branch, then opens the pull request into the default branch.",
    ),
    (
        "Enviada: lida no próximo passo",
        "Sent: read at its next step",
    ),
    ("Enviado", "Sent"),
    ("Enviado {}", "Pushed {}"),
    ("Enviado.", "Pushed."),
    ("Enviando…", "Pushing…"),
    ("Enviar", "Send"),
    ("Enviar alerta de teste", "Send a test alert"),
    (
        "Enviar e começar nova conversa",
        "Send and start new thread",
    ),
    ("Enviar mensagem", "Send message"),
    ("Enviar para segundo plano", "Send to background"),
    ("Enviar {} para o remoto?", "Push {} to the remote?"),
    (
        "Envie /compact sozinho, sem anexos e sem os modos equipe ou depuração.",
        "Send /compact on its own, without attachments, team or debug mode.",
    ),
    ("Equipe", "Team"),
    (
        "Escolha Sistema, Claro ou Escuro; a translucidez é uma preferência de material.",
        "Choose System, Light or Dark; translucency is a material preference.",
    ),
    (
        "Escolha como o Sirus avisa quando o trabalho termina ou precisa da sua atenção.",
        "Choose how Sirus alerts you when work finishes or needs your attention.",
    ),
    (
        "Escolha menos domínios para uma única importação.",
        "Pick fewer domains for one import.",
    ),
    (
        "Escolha no máximo dois provedores com uso para a barra lateral.",
        "Choose at most two providers with usage for the sidebar.",
    ),
    (
        "Escolha o escopo do usuário ou do projeto.",
        "Choose the user or project scope.",
    ),
    (
        "Escolha o provedor das novas sessões. As sessões atuais mantêm seu provedor.",
        "Choose the provider for new sessions. Current sessions keep their provider.",
    ),
    (
        "Escolha o que abrir ao lado da conversa.",
        "Choose what to open beside the conversation.",
    ),
    (
        "Escolha o workspace padrão das novas sessões.",
        "Choose the default workspace for new sessions.",
    ),
    (
        "Escolha o ícone exibido no Dock.",
        "Choose the icon shown in the Dock.",
    ),
    (
        "Escolha os padrões para novas sessões, navegação e o painel Ambiente.",
        "Choose defaults for new sessions, navigation and the Environment panel.",
    ),
    (
        "Escolha pelo menos um domínio para importar.",
        "Pick at least one domain to import.",
    ),
    (
        "Escolha pelo menos um provedor.",
        "Choose at least one provider.",
    ),
    (
        "Escolha se uma mensagem enviada enquanto o agente trabalha espera na fila ou instrui a resposta atual (Codex e Claude). ⌘Enter faz o contrário só naquela mensagem.",
        "Choose whether a message sent while the agent works waits in the queue or steers the current reply (Codex and Claude). ⌘Enter does the opposite for one message.",
    ),
    ("Escolha um aparelho", "Choose a device"),
    (
        "Escolha um horário no futuro, em até um ano.",
        "Choose a time in the future, within a year.",
    ),
    ("Escolha um item", "Pick an item"),
    (
        "Escolha um modelo para esta conversa: o Ask executa o Codex no modo de planejamento.",
        "Pick a model for this conversation: Ask runs Codex in planning mode.",
    ),
    (
        "Escolha um projeto para o escopo do projeto.",
        "Choose a project for the project scope.",
    ),
    (
        "Escolha uma das cores ou um valor #rrggbb.",
        "Choose one of the colours or a #rrggbb value.",
    ),
    (
        "Escolha uma pasta para salvar.",
        "Choose a folder to save into.",
    ),
    (
        "Escolha uma resposta concluída do assistente nesta conversa.",
        "Choose a finished assistant response from this session.",
    ),
    ("Escolher imagem", "Choose image"),
    ("Escolher outra pasta…", "Choose another folder…"),
    ("Escolher pasta", "Choose folder"),
    ("Escolher uma imagem", "Choose an image"),
    ("Escolher uma pasta…", "Choose a folder…"),
    ("Esconder arquivos ignorados", "Hide ignored files"),
    (
        "Escreva a meta e aperte Enter",
        "Write the goal and press Enter",
    ),
    ("Escreva um comentário…", "Write a comment…"),
    (
        "Escreva um pedido dentro do limite de 64 KiB, incluindo os anexos.",
        "Enter a request within the 64 KiB limit, including attachments.",
    ),
    (
        "Escreva um prompt no composer e depois copie o link.",
        "Write a prompt in the composer, then copy its link.",
    ),
    (
        "Escreva uma mensagem de até 4.000 caracteres.",
        "Write a message of up to 4,000 characters.",
    ),
    ("Escrita de cache", "Cache write"),
    ("Escuro", "Dark"),
    ("Esforço e velocidade", "Effort and speed"),
    (
        "Esforço gerenciado pelo provedor; esta CLI/modelo não expõe ajuste manual.",
        "Provider-managed effort; this CLI/model exposes no manual adjustment.",
    ),
    ("Espaço", "Space"),
    (
        "Espaço de trabalho temporário do título do commit indisponível",
        "Commit title temporary workspace unavailable",
    ),
    ("Espaçosa", "Spacious"),
    ("Esperando o primeiro passo…", "Waiting for its first step…"),
    (
        "Espere o turno atual terminar",
        "Wait for the current turn to finish",
    ),
    ("Esperou a página", "Waited for the page"),
    (
        "Essa conta do GitHub não está conectada.",
        "That GitHub account is not signed in.",
    ),
    (
        "Essa conta não está conectada. Entre nela em Configurações → Provedores primeiro.",
        "That account is not signed in. Sign in to it in Settings → Providers first.",
    ),
    (
        "Essa imagem tem detalhes demais para um logo.",
        "That image is too detailed for a logo.",
    ),
    (
        "Essa solicitação não está mais aguardando.",
        "That request is no longer waiting.",
    ),
    (
        "Esse campo do 1Password não é texto.",
        "That 1Password field is not text.",
    ),
    (
        "Esse computador não reconhece mais este Mac.",
        "That computer no longer recognises this Mac.",
    ),
    (
        "Esse computador perdeu o pareamento: pareie de novo.",
        "This computer lost its pairing: pair it again.",
    ),
    (
        "Esse dispositivo não está mais pareado.",
        "That device is no longer paired.",
    ),
    (
        "Esse item do 1Password não é válido.",
        "That 1Password item is not valid.",
    ),
    (
        "Esse perfil do navegador não está disponível.",
        "That browser profile is not available.",
    ),
    (
        "Esse processo já terminou.",
        "That process has already ended.",
    ),
    (
        "Esse processo não pertence a esta conversa, então ele continua em execução.",
        "That process does not belong to this conversation, so it is left running.",
    ),
    (
        "Esse provedor só roda com um acesso mais amplo do que esta conversa permite; escolha outro provedor ou altere o modo de acesso.",
        "That provider only runs with broader access than this conversation allows; pick another provider or change the access mode.",
    ),
    (
        "Esse servidor não está mais em execução.",
        "That server is no longer running.",
    ),
    (
        "Esses arquivos não fazem parte deste turno.",
        "Those files are not part of this turn.",
    ),
    (
        "Esta conta atingiu o limite de uso.",
        "This account reached its usage limit.",
    ),
    (
        "Esta conta atingiu o limite de uso. Volta às {}.",
        "This account reached its usage limit. It returns at {}.",
    ),
    (
        "Esta conta de provedor é desconhecida ou não é compatível.",
        "This provider account is unknown or unsupported.",
    ),
    (
        "Esta conversa já está sendo movida para outra conta.",
        "This conversation is already moving to another account.",
    ),
    (
        "Esta conversa já usa essa conta.",
        "This conversation already uses that account.",
    ),
    (
        "Esta conversa não está acompanhando um pull request.",
        "This session is not watching a pull request.",
    ),
    (
        "Esta conversa não está mais em execução.",
        "This conversation is no longer running.",
    ),
    (
        "Esta conversa não faz parte deste pull request.",
        "This conversation is not part of this pull request.",
    ),
    (
        "Esta conversa não tem uma origem do GitHub de um projeto salvo.",
        "This session has no GitHub origin of a saved project.",
    ),
    (
        "Esta conversa parou por um erro",
        "This conversation stopped on an error",
    ),
    (
        "Esta conversa é longa demais para editar a última mensagem. Envie uma mensagem nova.",
        "This conversation is too long to edit its last message. Send a new message instead.",
    ),
    (
        "Esta equipe não está em execução.",
        "This team is not running.",
    ),
    (
        "Esta imagem é grande demais para mostrar aqui.",
        "This image is too large to show here.",
    ),
    (
        "Esta instalação não tem atualização com um clique compatível.",
        "This installation has no supported one-click update.",
    ),
    ("Esta parte entra em seguida.", "This part comes next."),
    (
        "Esta pasta não é um repositório Git.",
        "This folder is not a Git repository.",
    ),
    (
        "Esta página está numa janela flutuante.",
        "This page is in a floating window.",
    ),
    (
        "Esta resposta não aceita instruções agora.",
        "This reply cannot take instructions now.",
    ),
    (
        "Esta saída não está mais disponível.",
        "This output is no longer available.",
    ),
    (
        "Esta skill está desativada no Sirus.",
        "This skill is disabled in Sirus.",
    ),
    (
        "Esta tarefa em segundo plano não está em execução.",
        "This background task is not running.",
    ),
    (
        "Esta transcrição excede o limite de contexto de bifurcação de 32 KiB. Use uma conversa mais curta ou escreva um resumo de transferência.",
        "This transcript exceeds the 32 KiB fork context limit. Use a shorter session or write a handoff summary.",
    ),
    (
        "Esta é a própria janela do Sirus Native, não um dispositivo.",
        "This is the Sirus Native window itself, not a device.",
    ),
    ("Estado atualizado.", "Status updated."),
    ("Este Mac", "This Mac"),
    (
        "Este PDF excede os limites de páginas ou imagens do leitor.",
        "This PDF exceeds the reader's page or image limits.",
    ),
    (
        "Este agente auxiliar não pode ser resolvido agora.",
        "This helper cannot be resolved right now.",
    ),
    (
        "Este anexo não está mais disponível. Anexe novamente.",
        "This attachment is no longer available. Attach it again.",
    ),
    (
        "Este anexo não é uma imagem.",
        "This attachment is not an image.",
    ),
    (
        "Este arquivo excede os limites de conteúdo do leitor. Use um documento menor.",
        "This file exceeds the reader's content limits. Use a smaller document.",
    ),
    (
        "Este branch não tem upstream em um remoto de onde puxar.",
        "This branch has no upstream on a remote to pull from.",
    ),
    (
        "Este branch é o branch padrão do repositório.",
        "This branch is the repository's default branch.",
    ),
    (
        "Este computador ainda não tem endereço. Defina a rota pública dele em remote-public.json.",
        "This computer has no address yet. Set its public route in remote-public.json.",
    ),
    ("Este código expirou", "This code expired"),
    (
        "Este diff excedeu o limite de armazenamento da revisão.",
        "This diff exceeded the review storage limit.",
    ),
    (
        "Este diff é grande demais para mostrar aqui. Abra-o no GitHub.",
        "This diff is too large to show here. Open it on GitHub.",
    ),
    (
        "Este documento está danificado, criptografado ou não é suportado.",
        "This document is damaged, encrypted or unsupported.",
    ),
    (
        "Este documento excede os limites de conteúdo do leitor.",
        "This document exceeds the reader's content limits.",
    ),
    (
        "Este documento não tem conteúdo legível.",
        "This document has no readable content.",
    ),
    (
        "Este item não pode ser movido para a Lixeira aqui.",
        "This entry cannot be moved to the Trash here.",
    ),
    (
        "Este link de prompt não foi aberto.",
        "This prompt link was not opened.",
    ),
    (
        "Este link é de outro repositório. Abra-o no projeto dele.",
        "This link belongs to another repository. Open it in its own project.",
    ),
    (
        "Este modelo não oferece suporte ao esforço selecionado.",
        "This model does not support the selected effort.",
    ),
    (
        "Este modo de aprovação não é compatível com este adaptador.",
        "This approval mode is not supported by this adapter.",
    ),
    (
        "Este não é um arquivo de cookies do Safari.",
        "This is not a Safari cookies file.",
    ),
    (
        "Este pedido já tentou iniciar. Para mudar o texto, cancele e adicione outro pedido.",
        "This request already attempted to start. To change its text, cancel it and add another request.",
    ),
    (
        "Este perfil pertence a conversas existentes. Adicione outra conta.",
        "This profile belongs to existing sessions. Add another account instead.",
    ),
    (
        "Este projeto não tem script desse tipo.",
        "This project has no script of that kind.",
    ),
    (
        "Este projeto não tem um remote do GitHub (origin): publique-o no GitHub antes de mover.",
        "This project has no GitHub remote (origin): publish it to GitHub before moving.",
    ),
    (
        "Este projeto não tem um remoto do GitHub (origin), então o outro computador não consegue obter os arquivos. Publique-o no GitHub primeiro.",
        "This project has no GitHub remote (origin), so the other computer cannot get its files. Publish it to GitHub first.",
    ),
    (
        "Este projeto não tem um repositório no GitHub.",
        "This project has no GitHub repository.",
    ),
    (
        "Este provedor não informa uso.",
        "This provider does not report usage.",
    ),
    (
        "Este provedor não tem modo somente leitura. Escolha outro provedor para o Ask.",
        "This provider has no read-only mode. Pick another provider for Ask.",
    ),
    (
        "Este repositório configura auxiliares de rede do Git. O push está indisponível porque a configuração do projeto poderia redirecionar credenciais ou conexões.",
        "This repository configures Git network helpers. Push is unavailable because project configuration could redirect credentials or connections.",
    ),
    (
        "Este repositório não é um dos seus projetos.",
        "This repository is not one of your projects.",
    ),
    (
        "Este servidor não foi iniciado pelo Sirus, então continua em execução.",
        "This server was not started by Sirus, so it is left running.",
    ),
    (
        "Este servidor não precisa de login.",
        "This server needs no sign-in.",
    ),
    (
        "Este serviço não oferece um login que o Sirus possa usar. Adicione-o com um token.",
        "This service does not offer a sign-in Sirus can use. Add it with a token instead.",
    ),
    (
        "Este serviço não permite que apps se registrem para login. Adicione-o com um token.",
        "This service does not let apps register for sign-in. Add it with a token instead.",
    ),
    (
        "Este subagente não está em execução.",
        "This subagent is not running.",
    ),
    (
        "Estes anexos já foram enviados.",
        "These attachments were already sent.",
    ),
    ("Estilo da barra lateral", "Sidebar style"),
    ("Estilo do cursor", "Cursor style"),
    ("Estrutura", "Outline"),
    ("Eventos e sons", "Events and sounds"),
    ("Excluir", "Delete"),
    ("Excluir conversa?", "Delete conversation?"),
    ("Excluir conversas?", "Delete conversations?"),
    ("Excluir o login de {}?", "Delete the login for {}?"),
    ("Excluir perfil", "Delete profile"),
    ("Excluir todas", "Delete all"),
    ("Excluir {}?", "Delete {}?"),
    ("Executar: {}", "Run: {}"),
    ("Executou", "Ran"),
    ("Executável", "Executable"),
    (
        "Exiba as ações do editor no painel Ambiente.",
        "Show the editor actions in the Environment panel.",
    ),
    (
        "Exiba o link do repositório no painel Ambiente.",
        "Show the repository link in the Environment panel.",
    ),
    ("Exibição", "Display"),
    ("Exibição da resposta", "Response streaming"),
    (
        "Explore todos os 100 componentes gratuitos com exemplos locais.",
        "Explore all 100 free components with local examples.",
    ),
    ("Explorou o projeto", "Explored the project"),
    ("Exportar conversa", "Export conversation"),
    ("Extra alto", "Extra high"),
    (
        "Falha ao gerar o título do commit",
        "Commit title generation failed",
    ),
    (
        "Falha ao gerar o título do commit ou o limite seguro foi excedido",
        "Commit title generation failed or exceeded its safe limits",
    ),
    ("Falha na saída do login.", "Sign-in output failed."),
    ("Falha no ditado.", "Dictation failed."),
    ("Falha no processo de login.", "Sign-in process failed."),
    ("Falhou", "Failed"),
    ("Fase {}", "Phase {}"),
    (
        "Faturas, pedidos e pagamentos",
        "Invoices, orders and payments",
    ),
    ("Favoritos", "Favorites"),
    (
        "Fazer deploy e gerenciar serviços",
        "Deploy and manage services",
    ),
    ("Fechada", "Closed"),
    ("Fechado", "Closed"),
    ("Fechados", "Closed"),
    ("Fechar", "Close"),
    ("Fechar aba", "Close tab"),
    ("Fechar abas à direita", "Close tabs to the right"),
    ("Fechar abas à esquerda", "Close tabs to the left"),
    ("Fechar edição", "Close editor"),
    ("Fechar o terminal?", "Close the terminal?"),
    ("Fechar os terminais?", "Close the terminals?"),
    ("Fechar outras abas", "Close other tabs"),
    ("Fechar painel", "Close pane"),
    ("Fechar todas as abas", "Close all tabs"),
    ("Fechar {}?", "Close {}?"),
    ("Fechou o navegador", "Closed the browser"),
    ("Ferramentas de desenvolvimento", "Developer tools"),
    ("Fila", "Queue"),
    ("Fixar", "Pin"),
    ("Fixar aba", "Pin tab"),
    ("Fixar ou desafixar a aba", "Pin or unpin the tab"),
    ("Fixar todas", "Pin all"),
    ("Fixo", "Fixed"),
    ("Floresta", "Forest"),
    (
        "Fluxos reutilizáveis encontrados nos provedores e no workspace atual.",
        "Reusable workflows found in providers and the current workspace.",
    ),
    ("Fonte da interface", "UI font"),
    ("Fonte de código", "Code font"),
    ("Fonte de entrada anterior", "Previous input source"),
    ("Fonte do terminal", "Terminal font"),
    ("Forte", "Strong"),
    (
        "Força do fundo antes de a conversa ter mensagens.",
        "Background strength before a chat has messages.",
    ),
    (
        "Força do fundo quando a conversa já tem mensagens. De 0 a 100%.",
        "Background strength once the conversation has messages. From 0 to 100%.",
    ),
    ("Funcionalidades experimentais", "Experimental features"),
    ("Fundo do chat", "Chat background"),
    ("Geral", "General"),
    ("Gerando um código…", "Making a code…"),
    ("Gerar código", "Generate code"),
    ("Gerar título do commit", "Generate commit title"),
    (
        "Gerar um título de commit exige um provedor Codex ou Claude instalado e ativado",
        "Generating a commit title requires an enabled installed Codex or Claude provider",
    ),
    (
        "Geração do título cancelada.",
        "Title generation cancelled.",
    ),
    (
        "Gerenciadores de senha, segurança do sistema, terminais e ferramentas de automação nunca podem ser controlados.",
        "Password managers, system security, terminals and automation tools can never be controlled.",
    ),
    ("Gerenciar perfis…", "Manage profiles…"),
    ("Girar", "Rotate"),
    ("Git detectado", "Git detected"),
    ("GitLab e Bitbucket", "GitLab and Bitbucket"),
    (
        "Globais (funcionam fora do Sirus)",
        "Global (work outside Sirus)",
    ),
    ("Gravar a página", "Record the page"),
    ("Gravação de Tela", "Screen Recording"),
    (
        "Gravação salva em Filmes › Sirus",
        "Recording saved in Movies › Sirus",
    ),
    ("Gravações do navegador", "Browser recordings"),
    ("HISTÓRICO", "HISTORY"),
    (
        "Head do pull request inválido.",
        "Invalid pull request head.",
    ),
    ("Histórico do Git incompleto", "Incomplete Git history"),
    ("Histórico · {}", "History · {}"),
    ("Hoje", "Today"),
    (
        "Hoje: {}% de {}% disponíveis",
        "Today: {}% of {}% available",
    ),
    ("Hora de renovar já passou", "Renewal time has passed"),
    (
        "Há agentes trabalhando. Reiniciar interrompe todos. As conversas e as mensagens ficam salvas, e a atualização também vale na próxima vez que você abrir o Sirus.",
        "Agents are working. Restarting interrupts all of them. Conversations and messages are kept, and the update also applies the next time you open Sirus.",
    ),
    (
        "Há comandos rodando em: {}. Fechar encerra esses shells e o que estiver rodando neles.",
        "Commands are running in: {}. Closing ends these shells and whatever runs in them.",
    ),
    (
        "Há conflitos com o branch base.",
        "There are conflicts with the base branch.",
    ),
    (
        "Há conflitos: resolva-os antes de preparar.",
        "There are conflicts: resolve them before staging.",
    ),
    (
        "Há filtros externos do Git configurados. As operações automáticas de status, diff e worktree estão indisponíveis porque os filtros podem executar comandos do projeto.",
        "External Git filters are configured. Automatic status, diff and worktree operations are unavailable because filters can execute project commands.",
    ),
    (
        "Há hooks do Git configurados. As operações automáticas do Git estão indisponíveis porque esses hooks podem executar comandos do projeto.",
        "Configured Git hooks are present. Automatic Git operations are unavailable because these hooks can execute project commands.",
    ),
    (
        "Há mais arquivos do que os listados.",
        "There are more files than listed.",
    ),
    (
        "Há temas do VS Code demais instalados.",
        "Too many VS Code themes are installed.",
    ),
    (
        "Há um comando rodando em \"{}\". Fechar encerra o shell e o que estiver rodando nele.",
        "A command is running in \"{}\". Closing ends the shell and whatever runs in it.",
    ),
    ("ID de modelo do Cursor inválido", "Invalid Cursor model ID"),
    (
        "Identidade da pergunta desconhecida",
        "Unknown question identity",
    ),
    ("Idioma", "Language"),
    ("Ignorado", "Skipped"),
    ("Imagem atual", "Current image"),
    ("Imagem de fundo", "Background image"),
    ("Indisponível", "Unavailable"),
    ("Informado pelo provedor", "Reported by the provider"),
    ("Informar", "Provide"),
    (
        "Informe um predefinido de dispositivo ou largura e altura.",
        "Give a device preset, or both width and height.",
    ),
    (
        "Informe um site, um usuário e uma senha.",
        "Enter a site, a username and a password.",
    ),
    ("Inicialização", "Startup"),
    ("Iniciando", "Starting"),
    ("Iniciando o Simulador…", "Starting the Simulator…"),
    (
        "Iniciar novos worktrees a partir do origin",
        "Start new worktrees from origin",
    ),
    (
        "Iniciar uma sessão nova quando o contexto estiver quase cheio",
        "Start a fresh session when the context is nearly full",
    ),
    ("Iniciou", "Started"),
    ("Inspetor da web", "Web Inspector"),
    (
        "Instala {} {} (MIT) com o seu npm, cerca de 40 MB. Nada é baixado antes de você pedir.",
        "Installs {} {} (MIT) with your npm, about 40 MB. Nothing is downloaded until you ask.",
    ),
    ("Instalado", "Installed"),
    ("Instalado · {}", "Installed · {}"),
    ("Instalados, desligados {}", "Installed, off {}"),
    ("Instalando…", "Installing…"),
    ("Instalar", "Install"),
    (
        "Instalar as ferramentas do aparelho",
        "Install the device tools",
    ),
    ("Instalação", "Installation"),
    (
        "Instale o 1Password CLI (brew install 1password-cli) e ligue \"Integrar com o 1Password CLI\" no app do 1Password.",
        "Install the 1Password CLI (brew install 1password-cli) and turn on \"Integrate with 1Password CLI\" in the 1Password app.",
    ),
    (
        "Instale o GitHub CLI (gh) para listar pull requests.",
        "Install the GitHub CLI (gh) to list pull requests.",
    ),
    (
        "Instale o GitLab CLI (`brew install glab`) e rode `glab auth login` para ver e criar merge requests.",
        "Install the GitLab CLI (`brew install glab`) and run `glab auth login` to see and open merge requests.",
    ),
    (
        "Instale o Node.js {} ou mais novo, com npm, e verifique de novo.",
        "Install Node.js {} or newer, with npm, and check again.",
    ),
    (
        "Instale o Xcode com um runtime de iOS e verifique de novo.",
        "Install Xcode with an iOS runtime and check again.",
    ),
    (
        "Instale outro provedor para fazer o handoff",
        "Install another provider to hand off",
    ),
    ("Instruir", "Steer"),
    ("Instruir agora", "Send now"),
    (
        "Integrações que disponibilizam modelos para sessões.",
        "Integrations that provide models for sessions.",
    ),
    (
        "Interrompa a conversa antes de movê-la para outro computador.",
        "Stop the conversation before moving it to another computer.",
    ),
    (
        "Interrompa o chat lateral desta conversa antes de excluí-lo.",
        "Stop this session's side chat before deleting it.",
    ),
    (
        "Interrompa ou conclua a conversa de origem antes de bifurcar.",
        "Stop or finish the source session before forking.",
    ),
    (
        "Interrompa ou conclua a conversa de origem antes de transferi-la.",
        "Stop or finish the source session before handing it off.",
    ),
    (
        "Interrompa ou conclua esta conversa antes de movê-la para outra conta.",
        "Stop or finish this conversation before moving it to another account.",
    ),
    (
        "Investigar erros, issues e traces",
        "Investigate errors, issues and traces",
    ),
    ("Início", "Home"),
    (
        "Início do histórico do Git inválido",
        "Invalid Git history start",
    ),
    ("Isso não pode ser desfeito.", "This can't be undone."),
    (
        "Isso não é um nome de domínio.",
        "That is not a domain name.",
    ),
    (
        "Issues do Jira e páginas do Confluence",
        "Jira issues and Confluence pages",
    ),
    (
        "Issues, pull requests e repositórios (token)",
        "Issues, pull requests and repositories (token)",
    ),
    (
        "Item do espaço de trabalho inválido.",
        "Invalid workspace entry.",
    ),
    ("Janela flutuante", "Floating window"),
    ("Janelas do app", "App windows"),
    (
        "Já existe um branch com esse nome.",
        "A branch with this name already exists.",
    ),
    (
        "Já existe um item com este nome.",
        "An entry with this name already exists.",
    ),
    (
        "Já existe um merge request para este branch.",
        "A merge request already exists for this branch.",
    ),
    (
        "Já existe um pull request para este branch.",
        "A pull request already exists for this branch.",
    ),
    (
        "Já existe um servidor com esse nome ali.",
        "A server with that name already exists there.",
    ),
    (
        "Já há outra solicitação de título de commit em execução",
        "Another commit title request is already running",
    ),
    (
        "Já há outro login de conta em andamento.",
        "Another account sign-in is already in progress.",
    ),
    (
        "Já pode usar o Sirus pelo telefone.",
        "You can use Sirus from your phone now.",
    ),
    ("Já usado por {}", "Already used by {}"),
    (
        "Leia antes de enviar. Nada foi enviado.",
        "Read it before you send. Nothing was sent.",
    ),
    (
        "Leia com a câmera do iPhone ou escaneie pelo app do Sirus.",
        "Scan with the iPhone camera or from the Sirus app.",
    ),
    ("Leitura de cache", "Cache read"),
    ("Leitura e busca", "Read / search"),
    ("Lendo o histórico…", "Reading the history…"),
    ("Lendo o que ele fez…", "Reading what it did…"),
    ("Lendo os limites…", "Reading the limits…"),
    ("Lenta", "Slow"),
    (
        "Ler os controles dos apps e enviar cliques e teclas.",
        "Read app controls and send clicks and keys.",
    ),
    ("Leu", "Read"),
    ("Leu a página", "Read the page"),
    ("Leva o contexto ao enviar", "Takes the context on send"),
    ("Liberados agora", "Allowed now"),
    ("Liberar espaço", "Free up space"),
    ("Ligada", "On"),
    ("Ligado", "On"),
    (
        "Ligado. Os novos QR codes abrem o endereço seguro; aparelhos pareados antes precisam de um novo código por lá.",
        "On. New QR codes open the secure address; devices paired before need a new code there.",
    ),
    ("Ligando {}…", "Booting {}…"),
    ("Ligar", "Turn on"),
    ("Lilás", "Lilac"),
    ("Limite de 5 horas", "5-hour limit"),
    (
        "Limite de armazenamento de anexos atingido. Remova os anexos não utilizados.",
        "Attachment storage limit reached. Remove unused attachments.",
    ),
    ("Limite de painéis atingido", "Pane limit reached"),
    ("Limite do histórico", "Scrollback limit"),
    ("Limite individual", "Individual limit"),
    ("Limite mensal", "Monthly limit"),
    ("Limite semanal", "Weekly limit"),
    ("Limites", "Limits"),
    ("Limpar busca", "Clear search"),
    ("Limpar cache", "Clear cache"),
    ("Limpar cookies", "Clear cookies"),
    ("Limpar desenho", "Clear drawing"),
    ("Limpar worktrees", "Clean up worktrees"),
    (
        "Limpar worktrees concluídos",
        "Clean up completed worktrees",
    ),
    (
        "Limpar worktrees que sobraram",
        "Clean up leftover worktrees",
    ),
    ("Limpeza", "Cleanup"),
    (
        "Linhas longas quebram em vez de rolar para o lado.",
        "Long lines wrap instead of scrolling sideways.",
    ),
    ("Link copiado", "Link copied"),
    ("Link copiado.", "Link copied."),
    (
        "Link do prompt copiado. Quem abrir recebe o prompt como rascunho; nada é enviado.",
        "Prompt link copied. Whoever opens it gets the prompt as a draft; nothing is sent.",
    ),
    (
        "Lista as sessões em que um agente está trabalhando ao lado do campo de mensagem, quando duas ou mais rodam ao mesmo tempo.",
        "Lists the sessions an agent is working on beside the composer, once two or more run at the same time.",
    ),
    ("Lista de conversas", "Conversation list"),
    ("Listou os apps", "Listed apps"),
    ("Local dos worktrees", "Worktree location"),
    ("Login", "Sign-in"),
    ("Login do serviço", "Service sign-in"),
    ("Login expirado", "Sign-in expired"),
    ("Login preenchido.", "Login filled in."),
    ("Logins salvos…", "Saved logins…"),
    ("Logo do projeto", "Project logo"),
    ("Logs de desenvolvimento", "Developer logs"),
    ("Luminosidade da cor", "Color lightness"),
    ("Mais antigas", "Older"),
    ("Mais escura ou mais clara.", "Darker or lighter."),
    ("Mantendo", "Keeping"),
    (
        "Mantenha no máximo 10 perfis personalizados.",
        "Keep at most 10 custom profiles.",
    ),
    (
        "Mantenha no máximo 20 anexos. Remova um antes de adicionar mais.",
        "Keep at most 20 attachments. Remove one before adding more.",
    ),
    ("Manter", "Keep"),
    ("Manter agentes prontos", "Keep agents ready"),
    (
        "Manter o Mac acordado enquanto um agente trabalha",
        "Keep the Mac awake while an agent works",
    ),
    (
        "Manter o Mac acordado para o celular",
        "Keep the Mac awake for the phone",
    ),
    (
        "Mantém a fonte escolhida guardada para quando esta opção estiver desligada.",
        "Keeps the chosen font saved for when this option is off.",
    ),
    ("Marcar como pronto", "Mark as ready"),
    ("Marcar na imagem", "Mark on the image"),
    (
        "Marcar {} como pronto para revisão?",
        "Mark {} as ready for review?",
    ),
    ("Mascote", "Mascot"),
    ("Materiais", "Materials"),
    ("Matiz da cor", "Color hue"),
    ("Matiz do fundo", "Background hue"),
    ("Medindo…", "Measuring…"),
    ("Memória", "Memory"),
    ("Mensagem (⌘↩ para commit)", "Message (⌘↩ to commit)"),
    ("Mensagem do commit", "Commit message"),
    ("Mensagem durante a resposta", "Message during a reply"),
    ("Mensagem para este subagente", "Message for this subagent"),
    ("Merge request criado", "Merge request opened"),
    (
        "Merge requests do GitLab e pull requests do Bitbucket aparecem no painel de alterações, com os checks, e podem ser criados por lá.",
        "GitLab merge requests and Bitbucket pull requests show in the Changes panel with their checks, and can be opened from there.",
    ),
    ("Mergeado", "Merged"),
    ("Mesclado", "Merged"),
    ("Mesclados", "Merged"),
    (
        "Mesclagem: arquivos alterados em relação ao primeiro pai.",
        "Merge: files changed against the first parent.",
    ),
    ("Mesclar", "Merge"),
    ("Mesclar {}?", "Merge {}?"),
    (
        "Mescle ou ignore todas as tarefas concluídas antes de remover os worktrees.",
        "Merge or skip every finished task before removing worktrees.",
    ),
    ("Meta", "Goal"),
    (
        "Metadados de histórico do Git inválidos ou excessivos",
        "Invalid or excessive Git history metadata",
    ),
    (
        "Mexa o mouse e clique no fundo",
        "Move the mouse and click the background",
    ),
    ("Mission Control", "Mission Control (macOS)"),
    ("Modelo", "Model"),
    ("Modelos do Cursor", "Cursor models"),
    ("Modelos, datasets e Spaces", "Models, datasets and Spaces"),
    ("Modo debug", "Debug mode"),
    ("Modo planejamento", "Planning mode"),
    (
        "Mostra as conversas do projeto como abas no topo da janela. Desligado, a barra mostra só a conversa atual.",
        "Show the project's conversations as tabs at the top of the window. Off, the title bar shows only the current conversation.",
    ),
    (
        "Mostra de novo a primeira entrada: seus agentes, um projeto e o visual do Sirus.",
        "Shows the first-run welcome again: your agents, a project and how Sirus looks.",
    ),
    (
        "Mostra um QR de uso único para conectar o app do Sirus no iPhone.",
        "Shows a one-time QR to connect the Sirus iPhone app.",
    ),
    (
        "Mostra um anel de consumo de até dois provedores no rodapé da barra lateral.",
        "Show a usage ring for up to two providers at the bottom of the sidebar.",
    ),
    ("Mostrar / ocultar barra lateral", "Toggle Sidebar"),
    ("Mostrar a mesa", "Show desktop"),
    ("Mostrar arquivos ignorados", "Show ignored files"),
    ("Mostrar arquivos não rastreados", "Show untracked files"),
    ("Mostrar cliques", "Show clicks"),
    ("Mostrar em", "Show on"),
    ("Mostrar mais {} arquivos", "Show {} more files"),
    ("Mostrar menos", "Show less"),
    ("Mostrar no Finder", "Show in Finder"),
    (
        "Mostrar o que há de novo depois de atualizar",
        "Show what's new after an update",
    ),
    (
        "Mostrar páginas que o agente abre",
        "Show pages the agent opens",
    ),
    ("Mostrar sessões trabalhando", "Show working sessions"),
    ("Mostrar teclas", "Show key presses"),
    ("Mostrar terminal ao rodar", "Show terminal when running"),
    ("Mostrar {} anteriores", "Show {} earlier"),
    ("Mostrar/ocultar Ambiente", "Toggle Environment"),
    (
        "Mostre notificações e toque sons também enquanto o Sirus estiver ativo.",
        "Also show system alerts and play sounds while Sirus is active.",
    ),
    (
        "Mostre um aviso para sessões que você não está visualizando.",
        "Show an in-app alert for sessions you are not currently viewing.",
    ),
    ("Movendo a conversa…", "Moving the conversation…"),
    ("Mover", "Move"),
    ("Mover \"{}\" para {}?", "Move \"{}\" to {}?"),
    (
        "Mover a pasta “{}” para a Lixeira?",
        "Move the folder “{}” to the Trash?",
    ),
    ("Mover para a Lixeira", "Move to Trash"),
    (
        "Mover para a Lixeira não está disponível nesta plataforma.",
        "Moving to the Trash is unavailable on this platform.",
    ),
    (
        "Mover para o espaço à direita",
        "Move to the space on the right",
    ),
    (
        "Mover para o espaço à esquerda",
        "Move to the space on the left",
    ),
    ("Mover para {}", "Move to {}"),
    (
        "Mover uma conversa envia um branch ao GitHub e exige confirmação.",
        "Moving a conversation pushes a branch to GitHub and needs confirmation.",
    ),
    ("Mover “{}” para a Lixeira?", "Move “{}” to the Trash?"),
    ("Mármore", "Marble"),
    ("Máximo", "Maximum"),
    ("Médio", "Medium"),
    ("Mínimo", "Minimal"),
    (
        "Na barra lateral, as conversas em que um agente trabalha ou espera sua aprovação ficam num grupo acima das datas.",
        "In the sidebar, conversations where an agent is working or waiting for your approval sit in a group above the dates.",
    ),
    ("Na fila", "Queue"),
    (
        "Na tomada, o Mac não entra em repouso e seus aparelhos sempre o alcançam. A tela continua apagando e bloqueando; na bateria, o repouso funciona normalmente. Fechar a tampa ainda põe o Mac em repouso.",
        "While plugged in, the Mac does not go to sleep, so your devices can always reach it. The screen still turns off and locks; on battery, sleep works as usual. Closing the lid still puts it to sleep.",
    ),
    ("Nada encontrado.", "Nothing matches."),
    ("Nada foi desfeito.", "Nothing was undone."),
    (
        "Nada foi registrado para este subagente.",
        "Nothing was recorded for this subagent.",
    ),
    ("Nada novo para preparar.", "Nothing new to stage."),
    (
        "Nada para o commit: não há alterações.",
        "Nothing to commit: there are no changes.",
    ),
    ("Nada por aqui.", "Nothing here."),
    ("Nada sobrando", "Nothing left over"),
    ("Nas mensagens", "In messages"),
    ("Navegador", "Browser"),
    ("Navegador do Sirus", "Sirus browser"),
    ("Navegador do sistema", "System browser"),
    ("Navegar", "Navigate"),
    (
        "Necessária para capturar a janela de outro app.",
        "Needed to capture another app's window.",
    ),
    ("Negar", "Deny"),
    ("Nenhum", "None"),
    ("Nenhum agente em execução.", "No agent is running."),
    (
        "Nenhum agente encontrado ainda. Instale um e ele aparece em Configurações → Provedores.",
        "No agent found yet. Install one and it shows up in Settings → Provedores.",
    ),
    (
        "Nenhum aparelho conectado ainda.",
        "No device connected yet.",
    ),
    (
        "Nenhum app liberado no momento.",
        "No app is allowed right now.",
    ),
    ("Nenhum arquivo alterado.", "No files changed."),
    ("Nenhum arquivo encontrado.", "No files found."),
    ("Nenhum commit ainda.", "No commits yet."),
    ("Nenhum conector encontrado.", "No connector matches."),
    (
        "Nenhum dispositivo de entrada de microfone está disponível.",
        "No microphone input device is available.",
    ),
    (
        "Nenhum dispositivo pareado tem este id.",
        "No paired device has this id.",
    ),
    ("Nenhum elemento foi marcado.", "No element was marked."),
    ("Nenhum executável", "No executable"),
    (
        "Nenhum favorito disponível. Marque um modelo com a estrela.",
        "No available favorites. Mark a model with the star.",
    ),
    ("Nenhum item no 1Password.", "No items in 1Password."),
    (
        "Nenhum login do GitHub está esperando.",
        "No GitHub sign-in is waiting.",
    ),
    ("Nenhum login salvo.", "No saved logins."),
    (
        "Nenhum modelo corresponde à busca.",
        "No model matches the search.",
    ),
    (
        "Nenhum modelo habilitado disponível neste provedor.",
        "No enabled models available from this provider.",
    ),
    ("Nenhum para limpar", "None to clean"),
    (
        "Nenhum projeto aqui corresponde a “{}”, então o rascunho está no projeto atual.",
        "No project here matches “{}”, so the draft is in the current project.",
    ),
    ("Nenhum provedor selecionado.", "No providers selected."),
    ("Nenhum pull request aberto.", "No open pull requests."),
    ("Nenhum pull request fechado.", "No closed pull requests."),
    ("Nenhum pull request mesclado.", "No merged pull requests."),
    ("Nenhum servidor local", "No local servers"),
    (
        "Nenhum simulador de iOS encontrado.",
        "No iOS simulator found.",
    ),
    ("Nenhum uso nesse período.", "No usage in this period."),
    ("Nenhuma", "None"),
    ("Nenhuma CLI detectada ainda.", "No CLI detected yet."),
    ("Nenhuma aba aberta", "No tabs open"),
    ("Nenhuma alteração", "No changes"),
    ("Nenhuma alteração.", "No changes."),
    ("Nenhuma atividade ainda.", "No activity yet."),
    ("Nenhuma ação ainda.", "No actions yet."),
    ("Nenhuma configuração encontrada", "No settings found"),
    (
        "Nenhuma conta do GitHub conectada.",
        "No GitHub account signed in.",
    ),
    ("Nenhuma conta encontrada", "No accounts found"),
    ("Nenhuma issue aberta.", "No open issues."),
    ("Nenhuma pasta criada", "No folders created"),
    ("Nenhuma skill encontrada", "No skills found"),
    ("Neste projeto · {}", "In this project · {}"),
    ("No Sirus", "In Sirus"),
    (
        "No botão de enviar, nos seus balões e na guia ativa.",
        "On the send button, your bubbles and the active tab.",
    ),
    (
        "No modo planejamento o agente pode olhar, mas não clicar nem digitar.",
        "In planning mode agents can look but not click or type.",
    ),
    (
        "No máximo dois provedores. Desligue um para escolher outro.",
        "Up to two providers. Turn one off to choose another.",
    ),
    (
        "No máximo quatro skills podem ser invocadas por turno.",
        "At most four skills may be invoked per turn.",
    ),
    ("Nome", "Name"),
    ("Nome da ação", "Action name"),
    (
        "Nome da conta, Enter para entrar",
        "Account name, Enter to sign in",
    ),
    ("Nome da pasta", "Folder name"),
    ("Nome de anexo inválido.", "Invalid attachment name."),
    ("Nome de branch inválido", "Invalid branch name"),
    ("Nome do arquivo", "File name"),
    ("Nome do novo branch", "New branch name"),
    ("Nome do perfil", "Profile name"),
    (
        "Nome inválido: use letras, números, /, ., - e _, sem espaços nem .. e sem começar com - ou terminar com / ou ponto.",
        "Invalid name: use letters, numbers, /, ., - and _, with no spaces or .., and not starting with - or ending with / or a dot.",
    ),
    (
        "Nome longo demais (120 caracteres no máximo).",
        "Name too long (120 characters at most).",
    ),
    (
        "Nomes de skills desativadas inválidos.",
        "Invalid disabled skill names.",
    ),
    ("Notificações", "Notifications"),
    ("Notificações do sistema", "System notifications"),
    ("Nova aba", "New tab"),
    ("Nova aba privada", "New private tab"),
    ("Nova conversa", "New conversation"),
    ("Nova conversa aqui", "New conversation here"),
    (
        "Nova conversa em worktree",
        "New conversation in a worktree",
    ),
    ("Nova pasta", "New folder"),
    ("Nova sessão", "New Session"),
    ("Novas abas abrem em", "New tabs open in"),
    ("Novas sessões", "New threads"),
    ("Novo", "New"),
    ("Novo arquivo", "New file"),
    ("Novo código", "New code"),
    ("Novo projeto", "New project"),
    ("Novo terminal", "New Terminal"),
    ("Novo worktree", "New worktree"),
    ("Não", "No"),
    (
        "Não consegui copiar os dados do Sirus.",
        "Could not copy the Sirus data.",
    ),
    (
        "Não consegui ligar o aparelho: {}",
        "Could not boot the device: {}",
    ),
    (
        "Não consegui preparar a cópia dos dados.",
        "Could not prepare the data copy.",
    ),
    ("Não detectado", "Not detected"),
    (
        "Não deu para escrever o título. Use /commit seguido da mensagem.",
        "Could not write the title. Use /commit followed by the message.",
    ),
    (
        "Não deu para falar com o 1Password.",
        "1Password could not be reached.",
    ),
    ("Não encontrado", "Not found"),
    ("Não encontrado no GitHub.", "Not found on GitHub."),
    ("Não encontrados {}", "Not found {}"),
    (
        "Não encontrei certificados do sistema.",
        "No system certificates were found.",
    ),
    (
        "Não foi possível abrir a janela de arquivos. Feche e abra o Sirus.",
        "The file window could not open. Quit and reopen Sirus.",
    ),
    (
        "Não foi possível abrir esta imagem.",
        "Could not open this image.",
    ),
    (
        "Não foi possível abrir este documento.",
        "Could not open this document.",
    ),
    (
        "Não foi possível abrir o Keychain.",
        "The Keychain could not be opened.",
    ),
    (
        "Não foi possível abrir o arquivo selecionado.",
        "Cannot open selected file.",
    ),
    (
        "Não foi possível abrir o navegador.",
        "Cannot open the browser.",
    ),
    ("Não foi possível abrir {}.", "Could not open {}."),
    (
        "Não foi possível acessar o Open VSX.",
        "Open VSX could not be reached.",
    ),
    (
        "Não foi possível acessar o registro de provedores.",
        "Cannot reach the provider registry.",
    ),
    ("Não foi possível anexar.", "Couldn't attach."),
    ("Não foi possível atualizar: {}", "Could not update: {}"),
    (
        "Não foi possível capturar a página.",
        "Could not capture the page.",
    ),
    (
        "Não foi possível capturar suas alterações não commitadas.",
        "Cannot snapshot your uncommitted changes.",
    ),
    (
        "Não foi possível converter essa imagem.",
        "Could not convert that image.",
    ),
    (
        "Não foi possível copiar os cookies do Chrome.",
        "Chrome's cookies could not be copied.",
    ),
    (
        "Não foi possível criar o perfil da conta",
        "Cannot create account profile",
    ),
    (
        "Não foi possível criar o perfil isolado da conta.",
        "Cannot create the isolated account profile.",
    ),
    (
        "Não foi possível criar uma pasta temporária.",
        "A temporary folder could not be created.",
    ),
    (
        "Não foi possível executar o git: {}",
        "Could not run git: {}",
    ),
    (
        "Não foi possível guardar o token do Bitbucket.",
        "The Bitbucket token could not be saved.",
    ),
    (
        "Não foi possível iniciar a verificação de atualização do provedor.",
        "Cannot start the provider update check.",
    ),
    (
        "Não foi possível iniciar o download do tema.",
        "Cannot start the theme download.",
    ),
    (
        "Não foi possível iniciar o login no provedor.",
        "Cannot start the provider sign-in.",
    ),
    (
        "Não foi possível iniciar o provedor de título de commit",
        "Cannot start commit title provider",
    ),
    (
        "Não foi possível inspecionar o arquivo selecionado.",
        "Cannot inspect selected file.",
    ),
    (
        "Não foi possível inspecionar o item selecionado.",
        "Cannot inspect selected entry.",
    ),
    (
        "Não foi possível ler a captura do anexo.",
        "Cannot read attachment snapshot.",
    ),
    (
        "Não foi possível ler a gravação.",
        "The recording could not be read.",
    ),
    (
        "Não foi possível ler a imagem da área de transferência.",
        "Cannot read clipboard image.",
    ),
    (
        "Não foi possível ler a resposta do registro de provedores.",
        "Cannot read the provider registry response.",
    ),
    (
        "Não foi possível ler as alterações do agente.",
        "Cannot read the worker's changes.",
    ),
    (
        "Não foi possível ler essa imagem.",
        "Could not read that image.",
    ),
    (
        "Não foi possível ler esta conversa.",
        "This conversation could not be read.",
    ),
    (
        "Não foi possível ler este PDF. Ele pode estar protegido por senha ou danificado.",
        "Could not read this PDF. It may be encrypted or damaged.",
    ),
    ("Não foi possível ler o HEAD", "Cannot read HEAD"),
    (
        "Não foi possível ler o arquivo selecionado.",
        "Cannot read selected file.",
    ),
    (
        "Não foi possível ler o commit.",
        "Could not read the commit.",
    ),
    ("Não foi possível ler o diff.", "Could not read the diff."),
    (
        "Não foi possível ler o histórico de uso.",
        "Usage history could not be read.",
    ),
    (
        "Não foi possível ler o item da pasta selecionada.",
        "Cannot read selected folder entry.",
    ),
    (
        "Não foi possível ler os cookies do Chrome.",
        "Chrome's cookies could not be read.",
    ),
    (
        "Não foi possível ler os cookies do Safari.",
        "Safari's cookies could not be read.",
    ),
    ("Não foi possível ler {}.", "Could not read {}."),
    (
        "Não foi possível listar a pasta selecionada.",
        "Cannot list selected folder.",
    ),
    (
        "Não foi possível montar o endereço da busca.",
        "Cannot build the search address.",
    ),
    (
        "Não foi possível mover este item para a Lixeira.",
        "Could not move this entry to the Trash.",
    ),
    (
        "Não foi possível preparar a captura do anexo.",
        "Cannot prepare attachment snapshot.",
    ),
    (
        "Não foi possível preparar a imagem anotada.",
        "Could not prepare the annotated image.",
    ),
    (
        "Não foi possível remover o perfil da conta",
        "Cannot remove account profile",
    ),
    (
        "Não foi possível renomear o perfil da conta",
        "Cannot rename account profile",
    ),
    (
        "Não foi possível salvar a conversa.",
        "Could not save the conversation.",
    ),
    (
        "Não foi possível salvar as alterações do agente auxiliar antes da limpeza.",
        "Cannot save a helper's changes before cleanup.",
    ),
    (
        "Não foi possível salvar o login.",
        "The sign-in could not be saved.",
    ),
    (
        "Não foi possível salvar o plano de fundo.",
        "Could not save the background.",
    ),
    (
        "Não foi possível selecionar o perfil da conta",
        "Cannot select account profile",
    ),
    ("Não foi possível verificar", "Could not check"),
    (
        "Não há commit para corrigir.",
        "There is no commit to amend.",
    ),
    (
        "Não há mensagem para editar.",
        "There is no message to edit.",
    ),
    (
        "Não há mensagem para enviar novamente.",
        "There is no message to send again.",
    ),
    ("Não há pasta pessoal.", "No home folder."),
    (
        "Não há perfil de navegador com esse nome.",
        "There is no browser profile with that name.",
    ),
    (
        "Não há plano proposto para iniciar.",
        "There is no proposed plan to start.",
    ),
    (
        "Não há solicitação nativa ativa para esta conversa",
        "No live native request for this session",
    ),
    ("Não instalado", "Not installed"),
    ("Não permitir", "Don't allow"),
    ("Não quebrar linhas", "Don't wrap lines"),
    (
        "Não é possível acessar o anexo selecionado.",
        "Cannot access selected attachment.",
    ),
    (
        "Não é possível preparar caminhos, diretórios e submódulos vinculados",
        "Linked paths, directories and submodules cannot be prepared",
    ),
    (
        "Não é um anexo de imagem do GitHub.",
        "Not a GitHub image attachment.",
    ),
    (
        "Não é um link sirus://new.",
        "It is not a sirus://new link.",
    ),
    (
        "Número de pull request ou issue inválido.",
        "Invalid pull request or issue number.",
    ),
    (
        "Número inválido de agentes a manter prontos.",
        "Invalid number of agents to keep ready.",
    ),
    (
        "O 1Password não respondeu a tempo.",
        "1Password did not answer in time.",
    ),
    (
        "O 1Password recusou o pedido.",
        "1Password refused the request.",
    ),
    (
        "O 1Password respondeu algo ilegível.",
        "1Password answered something unreadable.",
    ),
    (
        "O Bitbucket não devolveu o novo pedido.",
        "Bitbucket did not return the new request.",
    ),
    (
        "O Bitbucket não respondeu. Tente de novo.",
        "Bitbucket did not answer. Try again.",
    ),
    (
        "O Bitbucket precisa de login. Adicione um token em Configurações → Git.",
        "Bitbucket needs a sign-in. Add a token in Settings → Git.",
    ),
    (
        "O Bitbucket recusou: esta conta não pode abrir pull requests neste repositório.",
        "Bitbucket refused: this account cannot open pull requests in this repository.",
    ),
    (
        "O Chrome não tem cookies para esse perfil.",
        "Chrome has no cookies for that profile.",
    ),
    (
        "O Claude lista só as skills que combinam com o projeto, pelas dependências e pelo AGENTS.md. Uma linha Skills: a, b no AGENTS.md mantém essas. As outras continuam disponíveis no composer.",
        "Claude lists only the skills that fit the project, from its dependencies and AGENTS.md. A line Skills: a, b in AGENTS.md keeps those. The others stay available in the composer.",
    ),
    (
        "O Claude não interrompeu a tarefa a tempo.",
        "Claude did not stop the task in time.",
    ),
    (
        "O Claude não moveu o subagente a tempo.",
        "Claude did not move the subagent in time.",
    ),
    (
        "O Claude não recebeu a mensagem a tempo.",
        "Claude did not take the message in time.",
    ),
    (
        "O Claude resume a conversa ao chegar perto deste total de tokens, de 100000 a 1000000. Vazio segue o padrão do Claude Code. Muda quando compacta, não o tamanho da janela do modelo.",
        "Claude summarizes the conversation near this many tokens, from 100000 to 1000000. Empty follows Claude Code's default. This changes when it compacts, not the model's window.",
    ),
    ("O Git não respondeu.", "Git did not answer."),
    (
        "O GitHub CLI (gh) não está instalado. Instale com `brew install gh` e depois adicione uma conta aqui.",
        "GitHub CLI (gh) is not installed. Install it with `brew install gh`, then add an account here.",
    ),
    (
        "O GitHub CLI (gh) não está instalado. Instale com `brew install gh`.",
        "GitHub CLI (gh) is not installed. Install it with `brew install gh`.",
    ),
    (
        "O GitHub CLI não conseguiu alterar a conta.",
        "GitHub CLI could not switch the account.",
    ),
    (
        "O GitHub não pode mesclar este pull request agora (verificações, revisões ou conflitos).",
        "GitHub cannot merge this pull request now (checks, reviews or conflicts).",
    ),
    (
        "O GitHub não respondeu. Tente novamente.",
        "GitHub did not answer. Try again.",
    ),
    (
        "O GitHub não retornou o novo pull request.",
        "GitHub did not return the new pull request.",
    ),
    (
        "O GitHub recusou a mudança do estado de rascunho.",
        "GitHub refused to change the draft state.",
    ),
    (
        "O GitHub recusou esta alteração.",
        "GitHub refused this change.",
    ),
    (
        "O GitHub recusou este pull request (o branch foi enviado e é diferente da base?).",
        "GitHub refused this pull request (is the branch pushed and different from the base?).",
    ),
    (
        "O GitLab CLI (glab) não está instalado. Instale com `brew install glab` e rode `glab auth login`.",
        "The GitLab CLI (glab) is not installed. Install it with `brew install glab` and run `glab auth login`.",
    ),
    (
        "O GitLab CLI não está conectado. Rode `glab auth login` no terminal.",
        "GitLab CLI is not signed in. Run `glab auth login` in a terminal.",
    ),
    (
        "O GitLab não devolveu o novo pedido.",
        "GitLab did not return the new request.",
    ),
    (
        "O GitLab não respondeu. Tente de novo.",
        "GitLab did not answer. Try again.",
    ),
    (
        "O GitLab recusou: esta conta não pode abrir merge requests neste projeto.",
        "GitLab refused: this account cannot open merge requests in this project.",
    ),
    (
        "O HEAD do Git mudou desde esta resposta ({} para {}): houve commit, checkout, rebase ou pull. Nada será desfeito. Abra o diff e reverta à mão, ou use git revert no novo commit.",
        "Git HEAD moved since this reply ({} to {}): a commit, checkout, rebase or pull happened. Nothing is undone. Open the diff and revert it by hand, or use git revert on the new commit.",
    ),
    (
        "O Keychain não liberou a chave do Chrome. Permita o Sirus quando o macOS perguntar e tente de novo.",
        "The Keychain did not release Chrome's key. Allow Sirus when macOS asks, then try again.",
    ),
    (
        "O Keychain só existe no macOS.",
        "The Keychain only exists on macOS.",
    ),
    (
        "O Mac não entra em repouso sozinho enquanto uma conversa trabalha. A tela ainda apaga e bloqueia, e fechar a tampa ainda põe o Mac em repouso.",
        "The Mac does not sleep on its own while a conversation works. The screen still dims and locks, and closing the lid still puts the Mac to sleep.",
    ),
    (
        "O Open VSX está indisponível no momento.",
        "Open VSX is unavailable right now.",
    ),
    (
        "O Open VSX parou de responder.",
        "Open VSX stopped responding.",
    ),
    (
        "O Open VSX retornou detalhes de tema malformados.",
        "Open VSX returned malformed theme details.",
    ),
    (
        "O Open VSX retornou uma resposta de busca ilegível.",
        "Open VSX returned an unreadable search response.",
    ),
    (
        "O Open VSX retornou uma resposta ilegível.",
        "Open VSX returned an unreadable response.",
    ),
    (
        "O QR code do app aponta para este endereço.",
        "The app's QR code points to this address.",
    ),
    (
        "O Safari ainda não tem cookies. Abra o Safari uma vez e tente de novo.",
        "Safari has no cookies yet. Open Safari once, then try again.",
    ),
    (
        "O Simulador não está disponível",
        "The Simulator is not available",
    ),
    (
        "O Simulador não iniciou: {}",
        "The Simulator did not start: {}",
    ),
    (
        "O Simulador precisa do Node.js",
        "The Simulator needs Node.js",
    ),
    ("O Simulador precisa do Xcode", "The Simulator needs Xcode"),
    (
        "O Sirus abre o login do serviço no seu navegador; você entra uma vez.",
        "Sirus opens the service's sign-in in your browser; you sign in once.",
    ),
    ("O Sirus está sendo fechado", "Sirus is closing"),
    (
        "O Sirus precisa de Acesso Total ao Disco para ler os cookies do Safari.",
        "Sirus needs Full Disk Access to read Safari's cookies.",
    ),
    (
        "O Sirus preenche seu login salvo na página. O agente nunca vê a senha, e você não é perguntado de novo sobre este site nesta conversa.",
        "Sirus fills your saved login into the page. The agent never sees the password, and you are not asked again for this site in this conversation.",
    ),
    (
        "O Sirus removeu {} caractere(s) invisível(is), que podem esconder instruções para o agente. O link pode ser malicioso: leia o prompt antes de enviar.",
        "Sirus removed {} invisible character(s), which can hide instructions for the agent. The link may be malicious: read the prompt before you send it.",
    ),
    (
        "O acesso remoto ainda não está pronto.",
        "Remote access is not ready yet.",
    ),
    (
        "O acesso remoto já está em execução.",
        "Remote access is already running.",
    ),
    (
        "O acesso total não pode ser combinado com o modo de planejamento.",
        "Full access cannot be combined with planning mode.",
    ),
    (
        "O acompanhamento de PR está desativado em Configurações → Git.",
        "PR watch is turned off in Settings → Git.",
    ),
    (
        "O agente aceita apenas uma decisão para a sua solicitação",
        "The agent accepts only a decision for its request",
    ),
    (
        "O agente consulta, mas não altera o banco.",
        "The agent can query but not change the database.",
    ),
    ("O agente está trabalhando…", "The agent is working…"),
    ("O agente fez uma pergunta.", "The agent asked a question."),
    (
        "O agente quer entrar em {} como {}",
        "The agent wants to sign in to {} as {}",
    ),
    (
        "O agente só vê e usa os apps que você liberar, naquela sessão. Nada fica guardado depois que você fecha o app.",
        "Agents only see and use apps you allow, for that session. Nothing is remembered after you quit.",
    ),
    (
        "O agente trabalha dentro da pasta que você escolher.",
        "The agent works inside the folder you choose.",
    ),
    ("O anexo excede 10 MiB.", "Attachment exceeds 10 MiB."),
    (
        "O anexo não está mais disponível.",
        "Attachment is no longer available.",
    ),
    ("O anexo não tem arquivo.", "Attachment has no file."),
    (
        "O anexo selecionado não é uma pasta.",
        "Selected attachment is not a folder.",
    ),
    (
        "O arquivo está danificado, protegido por senha ou usa um formato não suportado.",
        "This file is damaged, encrypted or uses an unsupported format.",
    ),
    (
        "O arquivo mudou fora do editor. O salvamento automático parou; ⌘S substitui.",
        "The file changed outside the editor. Autosave stopped; ⌘S overwrites it.",
    ),
    (
        "O branch desta conversa não tem um pull request aberto no GitHub.",
        "This session's branch has no open pull request on GitHub.",
    ),
    (
        "O branch está desatualizado em relação ao base.",
        "The branch is behind the base.",
    ),
    (
        "O caminho do Git não está em UTF-8",
        "Git path is not UTF-8",
    ),
    (
        "O caminho do espaço de trabalho mudou",
        "Workspace path changed",
    ),
    (
        "O commit foi concluído, mas seus metadados não estão disponíveis. Atualize antes de continuar.",
        "Commit completed, but its metadata is unavailable. Refresh before continuing.",
    ),
    (
        "O computador demorou demais para responder.",
        "The computer took too long to answer.",
    ),
    (
        "O computador não devolveu a conversa.",
        "The computer did not return the conversation.",
    ),
    (
        "O computador não devolveu o projeto.",
        "The computer did not return the project.",
    ),
    (
        "O computador recusou o código.",
        "The computer refused the code.",
    ),
    (
        "O computador recusou o pedido.",
        "The computer refused the request.",
    ),
    (
        "O contexto da skill mudou. Revise a conversa atual e tente de novo.",
        "Skill context changed. Review the current session and try again.",
    ),
    (
        "O contexto da solicitação na fila mudou. Revise a fila antes de enviar novamente.",
        "Queued request context changed. Review the queue before sending again.",
    ),
    (
        "O conteúdo textual não mudou; o arquivo ou suas permissões foram alterados.",
        "The text content did not change; the file or its permissions changed.",
    ),
    (
        "O coordenador da equipe deve ser Codex, Claude Code ou OpenCode.",
        "A team coordinator must be Codex, Claude Code or OpenCode.",
    ),
    (
        "O coordenador não concluiu o plano da equipe.",
        "The coordinator did not finish the team plan.",
    ),
    (
        "O coordenador não retornou um plano de equipe.",
        "The coordinator did not return a team plan.",
    ),
    (
        "O coordenador retornou um plano de equipe inválido.",
        "The coordinator returned an invalid team plan.",
    ),
    (
        "O código de login expirou. Tente de novo.",
        "The sign-in code expired. Try again.",
    ),
    ("O código tem seis dígitos.", "The code has six digits."),
    (
        "O destino do espaço de trabalho está fora desta conversa.",
        "Workspace destination is outside this session.",
    ),
    (
        "O destino do espaço de trabalho mudou. Tente novamente.",
        "Workspace destination changed. Try again.",
    ),
    ("O diff foi truncado.", "The diff was truncated."),
    (
        "O diretório de dados está indisponível.",
        "Data directory is unavailable.",
    ),
    (
        "O diretório de logs não pode ser um link simbólico.",
        "Logs directory must not be a symlink.",
    ),
    (
        "O ditado só está disponível no macOS.",
        "Dictation is only available on macOS.",
    ),
    (
        "O encaixe dessa janela expirou.",
        "That window snap has expired.",
    ),
    (
        "O endereço HTTPS dele, como https://seu-computador.exemplo.com",
        "Its HTTPS address, such as https://your-computer.example.com",
    ),
    (
        "O endereço de login do serviço é inválido.",
        "The service's sign-in address is invalid.",
    ),
    (
        "O endereço de prévia deve ser uma URL http(s) de localhost com porta, como http://localhost:5173.",
        "The preview address must be a localhost http(s) URL with a port, like http://localhost:5173.",
    ),
    (
        "O endereço de qualquer servidor MCP remoto (HTTP).",
        "The address of any remote MCP server (HTTP).",
    ),
    (
        "O endereço desse computador é inválido.",
        "This computer's address is invalid.",
    ),
    ("O endereço não resolve.", "The address does not resolve."),
    (
        "O endereço precisa ser https.",
        "The address must be https.",
    ),
    (
        "O espaço de trabalho do PR mudou. Atualize para carregar o estado atual.",
        "The PR workspace changed. Refresh to load its current state.",
    ),
    (
        "O espaço de trabalho do coordenador mudou; tente de novo.",
        "The coordinator workspace changed; try again.",
    ),
    (
        "O espaço de trabalho está fora do repositório",
        "Workspace is outside repository",
    ),
    (
        "O espaço de trabalho mudou. Atualize antes de continuar.",
        "Workspace changed. Refresh before continuing.",
    ),
    (
        "O espaço de trabalho mudou; tente de novo.",
        "The workspace changed; try again.",
    ),
    (
        "O fetch automático está indisponível enquanto a configuração Git do projeto puder executar helpers ou divulgar credenciais. Faça o fetch manualmente pelo terminal.",
        "Automatic fetch is unavailable while project Git configuration can execute helpers or disclose credentials. Fetch manually in your terminal.",
    ),
    (
        "O fundo já muda enquanto você escolhe. Dá para trocar depois nas Configurações.",
        "The background already changes as you choose. You can change it later in Settings.",
    ),
    ("O git falhou.", "Git failed."),
    (
        "O glab não está conectado. Rode `glab auth login` no terminal.",
        "glab is not signed in. Run `glab auth login` in a terminal.",
    ),
    ("O host da URL não é permitido", "URL host is not allowed"),
    (
        "O item do espaço de trabalho está fora desta conversa.",
        "Workspace entry is outside this session.",
    ),
    (
        "O item do espaço de trabalho não existe mais.",
        "Workspace entry no longer exists.",
    ),
    (
        "O link escolheu {} para esta conversa.",
        "The link picked {} for this thread.",
    ),
    (
        "O link escolheu {}, mas o modelo não está disponível aqui; o modelo padrão é usado.",
        "The link picked {}, but its model is not available here; the default model is used.",
    ),
    ("O link não pôde ser lido.", "The link could not be read."),
    (
        "O link pediu {}, que não está configurado aqui; o seu padrão continua.",
        "The link asked for {}, which is not set up here; your default stays.",
    ),
    (
        "O login da conta está em andamento. Conclua-o antes de usar este perfil.",
        "Account sign-in is in progress. Complete it before using this profile.",
    ),
    (
        "O login da conta expirou. Tente novamente.",
        "Account sign-in timed out. Try again.",
    ),
    (
        "O login da conta foi cancelado.",
        "Account sign-in was cancelled.",
    ),
    (
        "O login de {} sai deste Mac. Conversas que usam esta conta impedem a remoção.",
        "{}'s sign-in leaves this Mac. Conversations using this account prevent removal.",
    ),
    (
        "O login demorou demais. Tente novamente.",
        "The sign-in took too long. Try again.",
    ),
    (
        "O login desta conta expirou ou foi sobrescrito por outra sessão.",
        "This account's sign-in expired or was overwritten by another session.",
    ),
    (
        "O login do GitHub não terminou.",
        "The GitHub sign-in did not finish.",
    ),
    ("O login não foi concluído.", "Sign-in was not completed."),
    ("O macOS já usa para {}", "macOS already uses this for {}"),
    (
        "O modelo de trabalho que percorre a borda do composer enquanto o agente trabalha.",
        "The working pattern that runs along the composer edge while the agent works.",
    ),
    (
        "O modelo não é válido ou não tem provedor.",
        "Its model is not valid, or has no provider.",
    ),
    (
        "O modelo, a conta ou o workspace mudou. Cancele os pedidos e envie-os novamente com a nova escolha.",
        "The model, account or workspace changed. Cancel the requests and submit them again with the new selection.",
    ),
    (
        "O modo de aprovação com que novas conversas começam. Um provedor sem ele usa o próximo modo mais seguro que oferece; uma conversa mantém o último modo usado.",
        "The approval mode new conversations start on. A provider without it uses the next safer mode it offers; a conversation keeps the mode it last used.",
    ),
    (
        "O modo de planejamento não é compatível com este adaptador.",
        "Planning mode is not supported by this adapter.",
    ),
    (
        "O modo rápido não é compatível com este adaptador/modelo.",
        "Fast mode is not supported by this adapter/model.",
    ),
    (
        "O motor desta janela, com seus projetos e arquivos.",
        "This window's engine, with your projects and files.",
    ),
    (
        "O motor do Sirus não respondeu.",
        "The Sirus engine did not respond.",
    ),
    (
        "O motor não gerou um código.",
        "The engine did not generate a code.",
    ),
    (
        "O nome da conta deve ter de 1 a 48 caracteres.",
        "Account name must contain 1–48 characters.",
    ),
    (
        "O nome deve ser uma única linha de texto.",
        "The name must be one line of text.",
    ),
    (
        "O nome do computador deve ter de 1 a 60 caracteres, em uma linha.",
        "The computer's name must have 1–60 characters on one line.",
    ),
    (
        "O nome precisa de pelo menos uma letra ou um número.",
        "The name needs at least one letter or digit.",
    ),
    (
        "O número do pull request não é válido.",
        "Its pull request number is not valid.",
    ),
    (
        "O objetivo deve ter no máximo 400 caracteres.",
        "Goal must contain at most 400 characters.",
    ),
    ("O pareamento foi recusado.", "Pairing was declined."),
    ("O pareamento não respondeu.", "Pairing did not respond."),
    (
        "O parâmetro de esforço do Cursor não está disponível",
        "Cursor effort parameter is unavailable",
    ),
    (
        "O perfil da conta deve ser privado ao usuário atual.",
        "The account profile must be private to the current user.",
    ),
    (
        "O perfil isolado da conta não existe.",
        "The isolated account profile is missing.",
    ),
    (
        "O planejamento exige um modelo selecionado.",
        "Planning requires a selected model.",
    ),
    (
        "O plano da equipe tem um caminho inválido.",
        "Team plan has an invalid path.",
    ),
    (
        "O plano da equipe tem um id de tarefa inválido.",
        "Team plan has an invalid task id.",
    ),
    (
        "O plano da equipe tem uma dependência inválida entre tarefas.",
        "Team plan has an invalid dependency between tasks.",
    ),
    (
        "O plano da equipe é grande demais.",
        "Team plan is too large.",
    ),
    (
        "O processo nativo terminou antes de aceitar a resposta",
        "Native process ended before accepting response",
    ),
    ("O projeto não é válido.", "Its project is not valid."),
    (
        "O prompt só tinha caracteres ocultos, então pode ser malicioso.",
        "Its prompt had only hidden characters, so it may be malicious.",
    ),
    (
        "O prompt tem mais de 8.000 caracteres.",
        "Its prompt is longer than 8,000 characters.",
    ),
    (
        "O provedor não aceita respostas interativas",
        "Provider does not accept interactive responses",
    ),
    (
        "O provedor não concluiu o login. Tente novamente.",
        "The provider did not complete sign-in. Try again.",
    ),
    (
        "O provedor não recebeu a instrução a tempo.",
        "The provider did not take the instruction in time.",
    ),
    (
        "O provedor retornou um título de commit inválido",
        "Provider returned an invalid commit title",
    ),
    (
        "O próprio agente para com o Parar da conversa.",
        "The agent itself stops with the conversation's Stop.",
    ),
    (
        "O próximo commit substitui o último, que ainda não foi enviado. Sem título, mantém a mensagem dele.",
        "The next commit replaces the last one, which isn't pushed yet. With no title, it keeps its message.",
    ),
    (
        "O pull request mudou no GitHub. Atualize e revise-o novamente.",
        "The pull request changed on GitHub. Refresh and review it again.",
    ),
    ("O que um aparelho pode fazer", "What a device can do"),
    ("O que vamos fazer em {}?", "What shall we do in {}?"),
    (
        "O rascunho excede o limite de 64 KiB",
        "Draft exceeds the 64 KiB limit",
    ),
    (
        "O reconhecimento de voz não está disponível para este idioma.",
        "Speech recognition is unavailable for this language.",
    ),
    (
        "O repositório não está no formato dono/nome.",
        "Its repository is not owner/name.",
    ),
    (
        "O responsável pela skill mudou. Atualize e tente de novo.",
        "Skill owner changed. Refresh and try again.",
    ),
    (
        "O responsável pelo anexo não existe mais.",
        "Attachment owner no longer exists.",
    ),
    (
        "O responsável pelo contexto não existe mais",
        "Context owner no longer exists",
    ),
    (
        "O responsável pelo rascunho do anexo não existe mais.",
        "Attachment draft owner no longer exists.",
    ),
    (
        "O responsável pelo rascunho não existe mais",
        "Draft owner no longer exists",
    ),
    (
        "O robô do projeto que anda sobre o campo de mensagem enquanto o agente trabalha.",
        "The project's robot that walks above the message field while the agent works.",
    ),
    (
        "O serviço não enviou um token de acesso.",
        "The service sent no access token.",
    ),
    (
        "O serviço não registrou o Sirus para login.",
        "The service did not register Sirus for sign-in.",
    ),
    (
        "O sirus-host não foi encontrado.",
        "sirus-host was not found.",
    ),
    (
        "O tema do VS Code escolhido não está instalado.",
        "The chosen VS Code theme is not installed.",
    ),
    (
        "O terminal usa sempre o shell configurado no sistema.",
        "The PTY always launches $SHELL. This cannot be changed.",
    ),
    (
        "O texto da busca deve ter de 1 a 256 caracteres.",
        "Search text must be 1 to 256 characters.",
    ),
    (
        "O trabalho da conversa, inclusive o que ainda não foi commitado, vai para uma branch sirus/move-… no GitHub do projeto. {} baixa essa branch e continua a conversa lá, com o texto até aqui. Esta conversa fica marcada como movida.",
        "The conversation's work, including what is not committed yet, goes to a sirus/move-… branch on the project's GitHub. {} fetches that branch and continues the conversation there, with the text so far. This conversation is marked as moved.",
    ),
    (
        "O turno de planejamento não foi concluído.",
        "The planning turn did not complete.",
    ),
    (
        "O título deve ter de 1 a 256 caracteres.",
        "The title must have 1 to 256 characters.",
    ),
    (
        "O título do commit exige alterações preparadas completas, sem conflitos e inalteradas, dentro deste espaço de trabalho",
        "Commit title requires unchanged complete conflict-free prepared changes inside this workspace",
    ),
    (
        "O uso do computador só está disponível no macOS",
        "Computer use is only available on macOS",
    ),
    (
        "O uso do computador só está disponível no macOS.",
        "Computer use is available on macOS only.",
    ),
    (
        "O valor de esforço do Cursor não está disponível",
        "Cursor effort value is unavailable",
    ),
    (
        "O worktree de origem não é mais um repositório Git.",
        "The source worktree is no longer a Git repository.",
    ),
    (
        "O worktree desta conversa não existe mais.",
        "That session's worktree no longer exists.",
    ),
    (
        "O ícone muda na Dock enquanto o Sirus está aberto; com o app fechado, a Dock mostra o padrão.",
        "The icon changes in the Dock while Sirus is open; when the app is closed, the Dock shows the default.",
    ),
    (
        "O último commit já está em um remoto, então não pode ser corrigido.",
        "The last commit is already on a remote, so it can't be amended.",
    ),
    ("Objetivo inválido.", "Invalid goal."),
    ("Observou", "Read"),
    ("Oceano", "Ocean"),
    ("Ocultar saída", "Hide output"),
    ("Olhos", "Eyes"),
    (
        "Onde abre um link de pull request ou issue de um dos seus projetos. Outros repositórios seguem Abrir links no.",
        "Where a link to a pull request or issue of one of your projects opens. Other repositories follow Open links in.",
    ),
    (
        "Onde abrem os links de respostas, das suas mensagens e dos terminais. ⌘, Ctrl, Option ou clique do meio sempre abrem o navegador do sistema.",
        "Where web links in replies, your messages and terminals open. ⌘, Ctrl, Option or middle click always opens the system browser.",
    ),
    (
        "Onde seus aparelhos encontram este Mac.",
        "Where your devices find this Mac.",
    ),
    (
        "Onde uma conversa nova trabalha: na pasta do projeto ou num worktree novo. Uma pasta sem Git trabalha sempre nela mesma.",
        "Where a new conversation works: in the project's folder or in a new worktree. A folder without Git always works in place.",
    ),
    ("Onde vale", "Where it applies"),
    ("Ontem", "Yesterday"),
    ("Opcional", "Optional"),
    ("Ordem da lista", "List order"),
    ("Ordem das sessões", "Thread order"),
    ("Ordem dos projetos", "Project order"),
    (
        "Organiza as sessões dentro de cada projeto. As fixadas mantêm sua própria área.",
        "Orders sessions inside each project. Pinned sessions keep their own area.",
    ),
    ("Organização da sidebar", "Sidebar organization"),
    (
        "Os agentes da equipe usam aprovações do Ask ou do Auto; o acesso total não está disponível para equipes.",
        "Team workers use Ask or Auto approvals; Full access is unavailable for teams.",
    ),
    (
        "Os agentes deixam de recebê-lo e o login dele é esquecido.",
        "Agents stop receiving it and its sign-in is forgotten.",
    ),
    (
        "Os anexos devem somar até 40 MiB.",
        "Attachments must total at most 40 MiB.",
    ),
    ("Os anexos excedem 40 MiB.", "Attachments exceed 40 MiB."),
    (
        "Os anexos não pertencem a esta conversa.",
        "Attachments do not belong to this conversation.",
    ),
    (
        "Os arquivos voltam a como estavam antes desta resposta. Um arquivo mudado de novo depois, nos mesmos trechos, fica como está. Nada é commitado nem preparado.",
        "Files go back to how they were before this reply. A file changed again since then in the same lines is left as it is. Nothing is committed or staged.",
    ),
    (
        "Os botões reagem ao ponteiro do mouse.",
        "Buttons react to the mouse pointer.",
    ),
    (
        "Os chats laterais acompanham a conversa.",
        "Side chats move with their conversation.",
    ),
    (
        "Os comentários devem ter de 1 a 65.536 bytes de texto.",
        "Comments must have 1 to 65,536 bytes of text.",
    ),
    (
        "Os cookies, logins e dados dos sites são removidos deste Mac, e as abas dele fecham.",
        "Its cookies, sign-ins and site data are removed from this Mac, and its tabs close.",
    ),
    (
        "Os detalhes do tema no Open VSX estão indisponíveis no momento.",
        "Open VSX theme details are unavailable right now.",
    ),
    (
        "Os diffs antigos foram liberados. O resumo e a confirmação foram preservados.",
        "Older diffs were released. The summary and acknowledgment are preserved.",
    ),
    (
        "Os logs devem ser arquivos regulares.",
        "Logs must be regular files.",
    ),
    (
        "Os metadados do Git não estão em UTF-8",
        "Git metadata is not UTF-8",
    ),
    (
        "Os modos de depuração e equipe não podem ser combinados.",
        "Debug and team modes cannot be combined.",
    ),
    (
        "Os modos de depuração e planejamento não podem ser combinados.",
        "Debug and planning modes cannot be combined.",
    ),
    (
        "Os nomes e ícones dos provedores indicam onde cada cópia foi encontrada. Skills invocadas ficam disponíveis entre provedores. Os switches controlam o catálogo do Sirus; não alteram a descoberta nas CLIs externas.",
        "Provider names and icons show where each copy was found. Invoked skills are available across providers. The switches control the Sirus catalog; they do not change discovery in the external CLIs.",
    ),
    (
        "Os pedidos aguardam você continuar a fila.",
        "Requests wait until you continue the queue.",
    ),
    (
        "Os pedidos começam em ordem após uma resposta concluída. A fila fica apenas nesta abertura do app.",
        "Requests start in order after a completed response. The queue lasts for this app run only.",
    ),
    (
        "Os scripts do projeto são executados somente no worktree da própria conversa.",
        "Project scripts run only in a session's own worktree.",
    ),
    (
        "Os seis dígitos de `sirus-host pair`, ou do QR code da página Conexões do outro Mac.",
        "The six digits from `sirus-host pair`, or from the other Mac's Connections page.",
    ),
    (
        "Os seletores de arquivo não estão disponíveis neste host.",
        "File pickers are not available on this host.",
    ),
    (
        "Os servidores MCP deste provedor não são gerenciados aqui.",
        "This provider's MCP servers are not managed here.",
    ),
    (
        "Ou diga ao agente o que fazer em vez disso (recusa a aprovação)…",
        "Or tell the agent what to do instead (declines the approval)…",
    ),
    ("Ou digite no app", "Or type in the app"),
    (
        "Outra conta não é compatível com este provedor, ou o limite de perfis foi atingido.",
        "Another account is not supported for this provider or the profile limit was reached.",
    ),
    (
        "Outra conversa ainda usa este worktree; exclua sem remover o worktree.",
        "Another session still works in this worktree; delete without removing it.",
    ),
    (
        "Outro app já usa esse atalho",
        "Another app already uses this shortcut",
    ),
    (
        "Outro documento está carregando. Tente novamente em instantes.",
        "Another document is loading. Try again in a moment.",
    ),
    (
        "Outro documento está carregando. Tente novamente.",
        "Another document is loading. Try again.",
    ),
    ("Outro limite", "Other limit"),
    ("Outros", "Others"),
    ("Outros modelos", "Other models"),
    ("Ouvir", "Listen"),
    ("PREPARADAS", "STAGED"),
    ("Padrão", "Default"),
    ("Padrão do nome da branch", "Branch naming pattern"),
    ("Padrões", "Defaults"),
    (
        "Pagamentos, catálogo e pedidos",
        "Payments, catalog and orders",
    ),
    ("Painel Ambiente", "Environment panel"),
    ("Painel lateral", "Side panel"),
    (
        "Painéis, menus e listas mudam sem deslizar, e o robô do composer para. Também vale quando Reduzir movimento está ligado nos Ajustes de Acessibilidade do Mac.",
        "Panels, menus and lists change without sliding, and the composer's robot stops. Also applies when Reduce motion is on in the Mac's Accessibility settings.",
    ),
    ("Palavra por palavra", "Token by token"),
    ("Paleta de comandos", "Command Palette"),
    ("Parada", "Stopped"),
    ("Parando…", "Stopping…"),
    ("Parar a gravação", "Stop recording"),
    ("Parar agente", "Stop Agent"),
    ("Parar conversa?", "Stop conversation?"),
    ("Parar e enviar", "Stop and send"),
    ("Parar e revogar tudo", "Stop and revoke all"),
    (
        "Parar e transcrever no composer, ou parar e enviar a mensagem assim que for transcrita.",
        "Stop and transcribe into the composer, or stop and send the message once it is transcribed.",
    ),
    ("Parar script", "Stop script"),
    ("Parar subagente", "Stop subagent"),
    (
        "Pare as conversas em andamento antes de remover este projeto.",
        "Stop the running conversations before removing this project.",
    ),
    ("Pare quando quiser", "Stop anytime"),
    ("Pareando…", "Pairing…"),
    ("Parear", "Pair"),
    ("Pareie de novo", "Pair again"),
    ("Participação", "Share"),
    ("Parágrafos prontos", "Finished paragraphs"),
    (
        "Passa a conversa a outro provedor nesta pasta. A recapitulação vai com a primeira mensagem.",
        "Hands the conversation to another provider in this folder. The recap goes with the first message.",
    ),
    ("Passou", "Passed"),
    ("Pasta", "Folder"),
    ("Pasta de skills do Sirus", "Sirus skills folder"),
    ("Pasta do projeto", "Project folder"),
    (
        "Pasta onde os worktrees são criados. Escolha uma pasta ou digite o caminho.",
        "Folder where worktrees are created. Choose a folder or type the path.",
    ),
    (
        "Pastas no local dos worktrees que nenhuma sessão usa. Pastas com alterações não commitadas ficam; branches nunca são apagadas.{}",
        "Folders in the worktree location that no session uses. Folders with uncommitted changes stay; branches are never deleted.{}",
    ),
    ("Pausada", "Paused"),
    ("Pausar fila", "Pause queue"),
    (
        "Pede confirmação antes de fechar ou sair enquanto os agentes estão em execução.",
        "Ask before closing or quitting while agents are running.",
    ),
    ("Pedidos de permissão", "Permission requests"),
    ("Pedir aprovação", "Request approval"),
    ("Pediu alterações", "Requested changes"),
    ("Pendente", "Pending"),
    ("Perfil de conta inválido.", "Invalid account profile."),
    (
        "Perfis personalizados precisam do macOS 14 ou mais recente.",
        "Custom profiles need macOS 14 or later.",
    ),
    ("Pergunta antes de agir.", "Asks before acting."),
    (
        "Pergunta antes de fechar uma aba do terminal; o shell e o histórico dela terminam.",
        "Ask before closing a terminal tab; its shell and history end.",
    ),
    ("Perguntar", "Ask"),
    ("Perguntar sempre", "Ask every time"),
    ("Perguntas do agente", "Questions from the agent"),
    ("Pergunte sem interromper", "Ask without interrupting"),
    ("Pergunte sobre a conversa", "Ask about the conversation"),
    ("Permissões do macOS", "macOS permissions"),
    ("Permissões padrão", "Default permissions"),
    (
        "Permite que agentes usem o navegador do app. Desligado, novos agentes não recebem as ferramentas dele e os que estão rodando são recusados; seu painel de navegador não muda.",
        "Allow agents to use the in-app browser. Off, new agents do not get its tools and running ones are refused; your own browser pane is unaffected.",
    ),
    ("Permitido", "Allowed"),
    ("Permitir", "Allow"),
    ("Permitir nesta sessão", "Allow for this session"),
    ("Permitir outros aparelhos", "Allow other devices"),
    (
        "Permitir que agentes controlem o Sirus",
        "Let agents control Sirus",
    ),
    (
        "Permitir que agentes usem o computador",
        "Allow agents to use the computer",
    ),
    (
        "Permitir que sessões acompanhem o pull request",
        "Let sessions watch their pull request",
    ),
    ("Personalizado", "Custom"),
    (
        "Personalização da barra inválida.",
        "Invalid rail customization.",
    ),
    (
        "Personalize materiais e movimento.",
        "Personalize materials and motion.",
    ),
    (
        "Personalize os comandos de teclado que você usa no aplicativo.",
        "Customize the keyboard commands you use in the app.",
    ),
    ("Período", "Time"),
    ("Pesquisar por \"{}\"", "Search for \"{}\""),
    ("Pessoal", "Personal"),
    ("Peça algo ao agente…", "Ask the agent something…"),
    ("Peça qualquer coisa", "Ask for anything"),
    ("Planejamento", "Planning"),
    ("Planejamento é só leitura", "Planning stays read-only"),
    (
        "Plano de fundo do chat inválido.",
        "Invalid chat background.",
    ),
    ("Pool da equipe", "Team pool"),
    ("Populares", "Popular"),
    ("Por modelo", "By model"),
    ("Por projeto", "By project"),
    (
        "Por segurança cada código vale 5 minutos e serve uma vez. Gere outro para continuar.",
        "For safety each code lasts 5 minutes and works once. Make another to continue.",
    ),
    ("Porta inválida.", "Invalid port."),
    ("Posição do terminal", "Terminal position"),
    ("Precisa de aprovação", "Needs approval"),
    ("Precisa de você", "Needs you"),
    ("Precisam entrar", "Need to sign in"),
    (
        "Predefinido de dispositivo desconhecido.",
        "Unknown device preset.",
    ),
    (
        "Preencher o código de {}",
        "Fill the authenticator code for {}",
    ),
    ("Preencher {}", "Fill {}"),
    (
        "Preferências de execução do modelo inválidas.",
        "Invalid model execution preferences.",
    ),
    ("Preferências padrão", "Core defaults"),
    ("Preparado", "Staged"),
    ("Preparados", "Staged"),
    ("Preparar", "Stage"),
    ("Preparar tudo", "Stage all"),
    ("Preparação", "Setup"),
    ("Pressione as teclas…", "Press the keys…"),
    ("Pressionou uma tecla", "Pressed a key"),
    ("Pressão de memória alta", "Memory pressure high"),
    ("Pressão de memória crítica", "Memory pressure critical"),
    ("Pressão de memória normal", "Memory pressure normal"),
    (
        "Primeiro dia com leitura: o ponto de partida é a primeira leitura de hoje.",
        "First day with a reading: the starting point is today's first reading.",
    ),
    ("Privado", "Private"),
    ("Processados", "Processed"),
    (
        "Procurando agentes instalados no seu Mac…",
        "Looking for agents installed on your Mac…",
    ),
    ("Procurando…", "Searching…"),
    (
        "Procurar atualizações automaticamente",
        "Check for updates automatically",
    ),
    ("Produtividade", "Productivity"),
    ("Programação", "Coding"),
    ("Projeto", "Project"),
    ("Projeto anterior", "Previous project"),
    (
        "Projeto do Supabase (project ref)",
        "Supabase project (project ref)",
    ),
    ("Projeto {}", "Project {}"),
    ("Projetos", "Projects"),
    (
        "Projetos e branches de Postgres serverless",
        "Serverless Postgres projects and branches",
    ),
    (
        "Projetos fixados ficam primeiro. Arrastar uma pasta ativa a ordem manual.",
        "Pinned projects stay first. Dragging a folder switches to manual order.",
    ),
    ("Projetos, deploys e logs", "Projects, deployments and logs"),
    (
        "Prompt adicionado a partir de um link",
        "Prompt added from a link",
    ),
    ("Pronto", "Ready"),
    ("Pronto para revisão", "Ready for review"),
    (
        "Proteção permanente. O Sirus não executa operações destrutivas do Git automaticamente.",
        "Cannot be turned off. Sirus never runs reset --hard, clean -fd, or worktree remove --force automatically.",
    ),
    ("Provedor", "Provider"),
    (
        "Provedor de título de commit indisponível",
        "Commit title provider unavailable",
    ),
    ("Provedor desconhecido.", "Unknown provider."),
    ("Provedor não reconhecido", "Unknown provider"),
    ("Provedor padrão", "Default provider"),
    ("Provedores", "Providers"),
    ("Prévia em HTML", "HTML preview"),
    ("Próxima fonte de entrada", "Next input source"),
    ("Próximo projeto", "Next project"),
    ("Pular", "Skip"),
    ("Pull exige confirmação.", "Pull requires confirmation."),
    ("Pull request criado", "Pull request created"),
    ("Pull request mesclado.", "Pull request merged."),
    (
        "Pull request não encontrado no GitHub.",
        "Pull request not found on GitHub.",
    ),
    (
        "Pull requests fixados inválidos.",
        "Invalid pinned pull requests.",
    ),
    (
        "Pull requests lidos inválidos.",
        "Invalid read pull requests.",
    ),
    (
        "Pull requests, acompanhamento de PRs, correção de CI e pushes por HTTPS usam a conta ativa. As contas ficam guardadas no GitHub CLI (gh).",
        "Pull requests, PR watches, CI fixes and HTTPS pushes use the active account. Accounts are kept by GitHub CLI (gh).",
    ),
    ("Puxa para {}", "Pulls into {}"),
    ("Puxando…", "Pulling…"),
    ("Puxar", "Pull"),
    ("Puxar agora", "Pull now"),
    ("Puxar da origem", "Pull from origin"),
    ("Páginas claras", "Light pages"),
    ("Páginas escuras", "Dark pages"),
    ("Quadros por segundo", "Frame rate"),
    ("Quadros, itens e atualizações", "Boards, items and updates"),
    (
        "Quando o agente da conversa aberta abre ou navega uma página, o painel do navegador abre para mostrá-la. Desligado, o painel fica como está.",
        "When the open conversation's agent opens or navigates a page, the browser pane opens to show it. Off, the pane stays as it is.",
    ),
    (
        "Quando os checks do PR aberto de uma sessão falham, o agente dela recebe os erros e corrige; depois o Sirus faz commit e push do branch da sessão (nunca forçado). Até 3 tentativas por falha; senão pausa e avisa.",
        "When the checks of a session's open pull request fail, its agent gets the errors and fixes them, then Sirus commits and pushes the session branch (never forced). Up to 3 attempts per failure; it pauses and notifies you otherwise.",
    ),
    (
        "Quando outra pessoa abre um pull request ou pede sua revisão no repositório de um projeto salvo. Verificado ao abrir o Sirus e quando você volta para ele.",
        "When someone else opens a pull request or asks for your review in a saved project's repository. Checked when Sirus opens and when you come back to it.",
    ),
    (
        "Quando um agente pede uma escolha ou resposta.",
        "When an agent asks you to choose or answer.",
    ),
    (
        "Quando um agente precisa de aprovação para continuar.",
        "When an agent needs approval to continue.",
    ),
    (
        "Quando um script inicia e o endereço responde, ele abre no navegador da sessão.",
        "When a script starts and the address answers, it opens in the session's browser.",
    ),
    (
        "Quando uma conversa do Claude ou do Codex chega a 80% da janela de contexto, a próxima mensagem inicia uma sessão nova do provedor com um resumo curto montado pelo próprio app: as duas últimas trocas palavra por palavra e uma linha para cada anterior. Nenhuma chamada ao modelo é gasta com isso, e a conversa completa continua aqui.",
        "When a Claude or Codex conversation reaches 80% of its context window, the next message starts a fresh provider session seeded with a short brief the app builds itself: the last two exchanges word for word and a line for each earlier one. No model call is spent on it, and the full transcript stays here.",
    ),
    (
        "Quando uma resposta termina, os passos dela se recolhem na linha \"Trabalhou por\". Desligue para manter todos os passos visíveis.",
        "When a reply ends, its steps fold into the \"Worked for\" line. Turn this off to keep every step visible.",
    ),
    (
        "Quando uma tarefa termina com sucesso. Cancelamentos e falhas não geram avisos.",
        "When a turn finishes successfully. Cancelled and failed turns do not notify.",
    ),
    (
        "Quantas conversas terminadas mantêm o agente aberto para responder mais rápido na próxima mensagem. Ficam até 30 minutos paradas; as paradas há mais tempo fecham primeiro.",
        "How many finished conversations keep their agent open to answer faster on the next message. They stay up to 30 minutes idle; the longest idle close first.",
    ),
    (
        "Quanto da cor aparece no fundo.",
        "How much of the color shows in the background.",
    ),
    ("Quebrar linhas", "Wrap lines"),
    ("Quebrar linhas nos diffs", "Wrap lines in diffs"),
    ("Raciocínio", "Thinking"),
    ("Rascunho", "Draft"),
    (
        "Reabre a sessão criada mais recentemente no último projeto. As outras sessões continuam na barra lateral; os agentes não iniciam automaticamente.",
        "Reopen the newest-created session in the last project. Other sessions remain in the sidebar; agents do not start automatically.",
    ),
    (
        "Reabre o último projeto ativo ao iniciar o Sirus.",
        "Automatically reopen the last active project when Sirus starts.",
    ),
    ("Reabrir", "Reopen"),
    ("Reabrir guia fechada", "Reopen closed tab"),
    ("Reabrir sessão mais nova", "Reopen newest session"),
    ("Reabrir {}?", "Reopen {}?"),
    ("Reabrir último projeto", "Reopen last project"),
    ("Reativar avisos", "Turn alerts back on"),
    ("Rebase e mesclar", "Rebase and merge"),
    ("Recarregar sem cache", "Reload without cache"),
    ("Recarregou", "Reloaded"),
    ("Recentes", "Recent"),
    ("Recolher tudo", "Collapse all"),
    ("Recolher turnos terminados", "Fold finished turns"),
    ("Recusar", "Decline"),
    ("Reduzir movimento", "Reduce motion"),
    ("Reiniciar agora", "Restart now"),
    ("Reiniciar e interromper", "Restart and interrupt"),
    ("Reiniciar para atualizar?", "Restart to update?"),
    (
        "Remove sessões encerradas e seus worktrees isolados. Worktrees com alterações são preservados.",
        "Removes isolated worktrees for completed, failed, or stopped sessions. Dirty trees are refused without --force.",
    ),
    ("Remover", "Remove"),
    ("Remover anexo?", "Remove attachment?"),
    ("Remover conta", "Remove account"),
    ("Remover da lista", "Remove from list"),
    ("Remover projeto?", "Remove project?"),
    ("Remover {}?", "Remove {}?"),
    ("Renomear", "Rename"),
    ("Renomear conta", "Rename account"),
    ("Renomear {}", "Rename {}"),
    ("Renova em {}", "Renews in {}"),
    ("Repositório", "Repository"),
    (
        "Responda a cada pergunta nativa exatamente uma vez",
        "Answer every native question exactly once",
    ),
    ("Responder", "Reply"),
    (
        "Resposta do Codex desatualizada ou de outra conversa",
        "Stale or foreign Codex response",
    ),
    ("Resposta incompleta.", "Incomplete reply."),
    (
        "Resposta inesperada do GitHub.",
        "Unexpected reply from GitHub.",
    ),
    (
        "Resposta inesperada do serviço.",
        "Unexpected reply from the service.",
    ),
    (
        "Resposta inesperada do título do commit.",
        "Unexpected commit title response.",
    ),
    ("Resposta inválida.", "Invalid reply."),
    (
        "Restaura a barra lateral e as larguras dos painéis.",
        "Clears sidebar collapse and panel widths in this session.",
    ),
    ("Restaurar", "Reset"),
    ("Restaurar configurações do Sirus", "Reset Sirus settings"),
    ("Restaurar layout da janela", "Reset window layout"),
    (
        "Restaurar os padrões desta página?",
        "Restore this page's defaults?",
    ),
    ("Restaurar padrões", "Restore defaults"),
    ("Restaurar preferências", "Reset preferences"),
    (
        "Restaurar preferências não apaga pastas de projetos. Requer confirmação.",
        "Resetting settings does not delete projects on disk. Confirmation is required.",
    ),
    ("Resumo", "Summary"),
    (
        "Retomar sozinho quando o limite de uso liberar",
        "Resume by itself when a usage limit resets",
    ),
    ("Revisar", "Review"),
    (
        "Revise o diff e faça commit.",
        "Review the diff and commit.",
    ),
    ("Revisores", "Reviewers"),
    (
        "Revisores poderão ser notificados.",
        "Reviewers may be notified.",
    ),
    ("Revisou", "Reviewed"),
    ("Revisão", "Review"),
    ("Revisão automática", "Auto-review"),
    ("Revisão solicitada", "Review requested"),
    ("Revogar", "Revoke"),
    (
        "Roda em um worktree novo antes do primeiro turno (até 10 minutos).",
        "Runs in a new worktree before its first turn (up to 10 minutes).",
    ),
    (
        "Roda no worktree sempre que um turno termina (até 2 minutos).",
        "Runs in the worktree each time a turn finishes (up to 2 minutes).",
    ),
    (
        "Roda só em um worktree isolado",
        "Runs only in an isolated worktree",
    ),
    (
        "Rodam no worktree próprio de cada sessão, como você, igual ao terminal. $SIRUS_PROJECT_ROOT é a pasta deste projeto.",
        "Run in each session's own worktree, as you, like the terminal. $SIRUS_PROJECT_ROOT is this project's folder.",
    ),
    ("Rodando 1 comando", "Running 1 command"),
    (
        "Rodando o script de preparação…",
        "Running the setup script…",
    ),
    (
        "Rodando o script de término…",
        "Running the on-finish script…",
    ),
    ("Rodando {} comandos", "Running {} commands"),
    ("Rodar ao terminar", "Run on finish"),
    ("Rodar de novo", "Run again"),
    ("Rodar preparação", "Run setup"),
    (
        "Rode `glab auth login` no terminal para ver e criar merge requests.",
        "Run `glab auth login` in a terminal to see and open merge requests.",
    ),
    ("Rode comandos no projeto.", "Run commands in the project."),
    ("Rode o app num aparelho.", "Run the app on a device."),
    ("Rolou", "Scrolled"),
    ("Rosa", "Pink"),
    ("Rosto", "Face"),
    ("Rápida", "Fast"),
    ("Rótulo da conta", "Account label"),
    ("Sair", "Sign out"),
    (
        "Salva contagens limitadas do ciclo de execução localmente. Prompts, output, paths e credenciais são excluídos.",
        "Stores bounded lifecycle counts locally. Prompts, output, paths and credentials are excluded.",
    ),
    ("Salvar", "Save"),
    ("Salvar automaticamente", "Save automatically"),
    ("Salvar login", "Save login"),
    ("Salvar login de {}?", "Save login for {}?"),
    ("Salvar scripts", "Save scripts"),
    ("Salvar, agentes podem usar", "Save, agents may use"),
    (
        "Salve um comando do projeto para rodar com um clique.",
        "Save a project command to run with one click.",
    ),
    (
        "Salvo. Digite outro para trocar, ou um traço para remover",
        "Saved. Type a new one to replace it, or a dash to remove it",
    ),
    ("Saturação do fundo", "Background saturation"),
    ("Saída", "Output"),
    ("Script de preparação concluído", "Setup script finished"),
    ("Script de preparação falhou", "Setup script failed"),
    ("Script de preparação interrompido", "Setup script stopped"),
    (
        "Script de preparação passou do tempo",
        "Setup script ran out of time",
    ),
    ("Script de término concluído", "On-finish script finished"),
    ("Script de término falhou", "On-finish script failed"),
    ("Script de término interrompido", "On-finish script stopped"),
    (
        "Script de término passou do tempo",
        "On-finish script ran out of time",
    ),
    (
        "Se o Sirus fechar inesperadamente enquanto um agente trabalha, a conversa continua na próxima abertura, pedindo que o agente confira primeiro o que já aconteceu. No máximo três vezes seguidas; conversas paradas, adiadas ou aguardando você ficam como estão.",
        "If Sirus closes unexpectedly while an agent works, that conversation continues on the next launch, asking the agent to check what already happened first. At most three times in a row; stopped, snoozed and waiting conversations are left alone.",
    ),
    (
        "Se outro app já usa a combinação, escolha outra.",
        "If another app already uses the combination, choose another.",
    ),
    (
        "Segredo do autenticador (opcional)",
        "Authenticator secret (optional)",
    ),
    ("Seguir o sistema", "Follow the system"),
    ("Segurança", "Safety"),
    ("Selecionar elemento", "Select an element"),
    (
        "Selecione um fluxo para adicionar /nome-da-skill ao rascunho.",
        "Select a workflow to add /skill-name to your draft.",
    ),
    (
        "Selecione um modelo antes de ativar o modo de planejamento.",
        "Select a model before enabling planning mode.",
    ),
    (
        "Selecione um pull request ou issue na lista.",
        "Select a pull request or issue from the list.",
    ),
    (
        "Seleção de provedor de uso inválida.",
        "Invalid usage provider selection.",
    ),
    ("Sem", "None"),
    (
        "Sem conexão com {}. Tentando de novo…",
        "No connection to {}. Trying again…",
    ),
    (
        "Sem conexão, tentando de novo",
        "No connection, trying again",
    ),
    ("Sem conversas", "No conversations"),
    ("Sem descrição informada.", "No description provided."),
    ("Sem descrição.", "No description."),
    ("Sem diferenças para mostrar.", "No differences to show."),
    ("Sem endereço para este Mac", "No address for this Mac"),
    ("Sem esforço", "None"),
    ("Sem limites informados.", "No limits reported."),
    ("Sem login", "Not signed in"),
    ("Sem resposta", "No reply"),
    ("Sem saída.", "No output."),
    ("Semanal · Opus", "Weekly · Opus"),
    ("Semanal · Sonnet", "Weekly · Sonnet"),
    ("Semanal · apps OAuth", "Weekly · OAuth apps"),
    ("Semanal · modelo reserva", "Weekly · reserve model"),
    ("Sempre bloqueados", "Always blocked"),
    (
        "Sempre ligado: excluir uma conversa apaga o histórico dela.",
        "Always on: deleting a conversation removes its history.",
    ),
    ("Senha", "Password"),
    ("Servidores locais", "Local servers"),
    ("Sessão", "Session"),
    (
        "Sessões trabalhando, esperando você ou concluídas sem serem vistas ficam acima da ordem das sessões. As listas não se mexem enquanto o ponteiro está sobre elas.",
        "Sessions that are working, waiting for you or finished unseen go above the thread order. Lists hold still while your pointer is over them.",
    ),
    ("Seu texto foi mantido.", "Your text was kept."),
    ("Seus agentes", "Your agents"),
    (
        "Seus agentes de código lado a lado, com as conversas e as mudanças num lugar só.",
        "Your coding agents side by side, with conversations and changes in one place.",
    ),
    ("Shell padrão", "Default shell"),
    ("Silenciar até reativar", "Mute until turned back on"),
    ("Silenciar por 1 hora", "Mute for 1 hour"),
    ("Silenciar por 4 horas", "Mute for 4 hours"),
    ("Silenciar por 8 horas", "Mute for 8 hours"),
    ("Sim", "Yes"),
    ("Simulador", "Simulator"),
    (
        "Sirus {} disponível · Instalar",
        "Sirus {} available · Install",
    ),
    ("Sistema", "System"),
    ("Sites, lojas e agendamentos", "Sites, stores and bookings"),
    ("Sites, páginas e CMS", "Sites, pages and CMS"),
    ("Skills compartilhadas", "Shared skills"),
    ("Skills conforme o projeto", "Skills by project"),
    ("Skills do projeto", "Project skills"),
    ("Skills dos agentes", "Agent skills"),
    ("Skills portáteis", "Portable skills"),
    ("Slide {} de {}", "Slide {} of {}"),
    ("Sobre", "About"),
    ("Solicitar aprovação", "Request approval"),
    ("Solicitação de saída inválida.", "Invalid output request."),
    (
        "Solicitação de título de commit inválida",
        "Invalid commit title request",
    ),
    (
        "Somente alterações fora dos caminhos planejados podem ser mescladas assim mesmo.",
        "Only changes outside the planned paths can be merged anyway.",
    ),
    (
        "Somente arquivos regulares podem ser anexados.",
        "Only regular files can be attached.",
    ),
    (
        "Somente conversas do Claude e do Codex podem continuar em outra conta.",
        "Only Claude and Codex conversations can continue on another account.",
    ),
    ("Somente leitura", "Read-only"),
    (
        "Somente servidores localhost podem ser interrompidos.",
        "Only localhost servers can be stopped.",
    ),
    (
        "Somente um plano proposto pode ser descartado.",
        "Only a proposed plan can be discarded.",
    ),
    (
        "Somente uma mesclagem interrompida pode ser ignorada ou forçada.",
        "Only a stopped merge can be skipped or forced.",
    ),
    (
        "Somente uma tarefa interrompida por um conflito pode ser resolvida.",
        "Only a task that stopped on a conflict can be resolved.",
    ),
    ("Sons", "Sounds"),
    ("Sons de notificação", "Notification sounds"),
    ("Sortear visual", "Draw another look"),
    ("Spotlight", "Spotlight (macOS)"),
    ("Squash e mesclar", "Squash and merge"),
    ("Status do Git inválido", "Invalid Git status"),
    (
        "Sua conta do GitHub não pode alterar este repositório.",
        "Your GitHub account cannot change this repository.",
    ),
    ("Suave", "Smooth"),
    ("Suavização de fontes", "Font smoothing"),
    ("Subagente desconhecido.", "Unknown subagent."),
    ("Subagente {}", "Subagent {}"),
    ("Sublinhado", "Underline"),
    (
        "Sublinhado de ortografia e correções automáticas do macOS enquanto você escreve uma mensagem, em todas as conversas.",
        "macOS spelling underlines and automatic corrections while you write a message, in every conversation.",
    ),
    ("Substituindo imagem…", "Replacing image…"),
    ("Substituir", "Replace"),
    ("Substituir original", "Replace original"),
    ("Sírius", "Sirius"),
    (
        "Só aparece nas listas do Sirus. Os agentes continuam usando o mesmo identificador.",
        "Only shown in Sirus lists. Agents keep using the same identifier.",
    ),
    (
        "Só conversas do Codex e do Claude Code podem ser compactadas.",
        "Only Codex and Claude Code conversations can be compacted.",
    ),
    ("Só em um projeto", "Only in one project"),
    (
        "Só endereços http e https abrem aqui.",
        "Only http and https addresses open here.",
    ),
    (
        "Só nas sessões vazias, ou em todas as conversas.",
        "Only in empty sessions, or in every conversation.",
    ),
    ("Só vazias", "Empty only"),
    ("Tamanho da fonte da interface", "UI font size"),
    ("Tamanho da fonte de código", "Code font size"),
    ("Tamanho da fonte do terminal", "Terminal font size"),
    ("Tamanho da interface", "Interface size"),
    ("Tamanho de aparelho", "Device size"),
    ("Tamanho personalizado", "Custom size"),
    ("Tamanho real", "Actual size"),
    (
        "Também respeita prefers-reduced-motion do sistema.",
        "Also follows the system's prefers-reduced-motion.",
    ),
    ("Tarefa", "Task"),
    ("Tarefa concluída", "Task completed"),
    (
        "Tarefa em segundo plano desconhecida.",
        "Unknown background task.",
    ),
    ("Tarefas, projetos e metas", "Tasks, projects and goals"),
    (
        "Teclas digitadas na página aparecem embaixo do vídeo. Campos de senha nunca aparecem.",
        "Keys typed on the page appear under the video. Password fields never appear.",
    ),
    ("Tem uma pergunta", "Has a question"),
    ("Tema", "Theme"),
    (
        "Tema do VS Code inválido: id duplicado.",
        "Invalid VS Code theme: duplicate id.",
    ),
    ("Tema do workspace", "Workspace theme"),
    ("Tentar de novo", "Try again"),
    ("Tentar novamente", "Try again"),
    ("Terminou", "Finished"),
    ("Testar notificação", "Test notification"),
    ("Teste", "Test"),
    (
        "Teste de notificação. Seus avisos de atividade aparecerão aqui.",
        "Notification test. Your activity alerts will appear here.",
    ),
    ("Texto da página", "Page text"),
    (
        "Texto dos slides. Imagens e layout não são exibidos.",
        "Slide text. Images and layout are not shown.",
    ),
    (
        "Texto e tabelas. Imagens e diagramação original não são exibidas.",
        "Text and tables. Images and original page layout are omitted.",
    ),
    ("Tinta", "Ink"),
    ("Tipografia da interface", "Interface typography"),
    ("Tipografia de código", "Code typography"),
    ("Tipografia do terminal", "Terminal typography"),
    ("Tirar da pasta", "Remove from folder"),
    ("Todas", "All"),
    ("Todas as conversas", "All conversations"),
    ("Todas as sessões", "All sessions"),
    ("Todo o período", "All time"),
    ("Todos", "All"),
    ("Todos os checkouts", "All checkouts"),
    ("Todos os projetos", "All projects"),
    ("Token guardado no Keychain.", "Token kept in the Keychain."),
    ("Tokens acumulados", "Cumulative tokens"),
    ("Tokens por tipo", "Tokens by type"),
    (
        "Toque o som escolhido. Usa o volume de saída do sistema.",
        "Play the selected cue. Uses your system output volume.",
    ),
    ("Trabalhando", "Working"),
    (
        "Trabalhando e não lidas primeiro",
        "Working and unread first",
    ),
    (
        "Trabalhando ou esperando você",
        "Working or waiting for you",
    ),
    (
        "Trabalhando ou esperando você primeiro",
        "Working or waiting for you first",
    ),
    ("Translucidez", "Translucency"),
    ("Trazer de volta ao painel", "Bring back to the pane"),
    ("Trocar", "Change"),
    (
        "Trocar de branch exige confirmação.",
        "Switching branches requires confirmation.",
    ),
    ("Trocar de projeto", "Switch project"),
    ("Trocar filtro", "Switch filter"),
    ("Trocar modelo e esforço", "Change model and effort"),
    ("Trocar método", "Change method"),
    ("Trocar token", "Replace token"),
    ("Trocar…", "Switch…"),
    ("Trouxe para a frente", "Brought to front"),
    ("Tudo", "All"),
    ("Tudo pronto", "All set"),
    ("Título do commit", "Commit title"),
    ("Título do pull request", "Pull request title"),
    ("Título escrito por {}", "Title written by {}"),
    (
        "Um agente da equipe está em execução de novo; aguarde a conclusão.",
        "A team worker is running again; wait for it to finish.",
    ),
    (
        "Um agente da equipe não pode coordenar outra equipe.",
        "A team worker cannot coordinate another team.",
    ),
    (
        "Um aparelho conectado pode fazer o que esta janela faz: abrir sessões, enviar mensagens, aprovar ações, fazer commit e push. Seletores de arquivo, ditado e controle do computador ficam só no Mac. Remova o aparelho que não usar mais.",
        "A connected device can do what this window does: start sessions, send messages, approve actions, commit and push. Pickers, dictation and computer use stay on the Mac. Remove a device you no longer use.",
    ),
    (
        "Um chat lateral não pode abrir outro chat lateral.",
        "A side chat cannot open another side chat.",
    ),
    (
        "Um chat lateral não pode coordenar uma equipe.",
        "A side chat cannot coordinate a team.",
    ),
    (
        "Um chat lateral ou uma conversa do Ask não pode acompanhar um pull request; use uma conversa principal.",
        "A side chat or Ask conversation cannot watch a pull request; use a main session.",
    ),
    (
        "Um chat lateral usa o espaço de trabalho da sua conversa principal; ele não pode removê-lo.",
        "A side chat uses its main session's workspace; it cannot remove it.",
    ),
    (
        "Um círculo marca cada clique no vídeo.",
        "A ring marks each click in the video.",
    ),
    (
        "Um endereço localhost que o script serve, como http://localhost:5173.",
        "A localhost address the script serves, such as http://localhost:5173.",
    ),
    (
        "Um plano de equipe precisa de 1 a 3 tarefas.",
        "A team plan needs 1 to 3 tasks.",
    ),
    (
        "Um script do projeto deve ser um texto de no máximo 8 KiB.",
        "A project script must be text of at most 8 KiB.",
    ),
    (
        "Um script do projeto já está em execução nesta conversa.",
        "A project script is already running in this session.",
    ),
    ("Um script já está rodando", "A script is already running"),
    (
        "Um toque suave quando um agente termina um turno ou precisa de você. Não toca para a conversa que você está vendo.",
        "A soft chime when an agent finishes a turn or needs you. Not played for the conversation you are viewing.",
    ),
    (
        "Uma conversa do Ask não pode coordenar uma equipe nem depurar.",
        "An Ask conversation cannot coordinate a team or debug.",
    ),
    (
        "Uma conversa do Ask não pode ser bifurcada.",
        "An Ask conversation cannot be forked.",
    ),
    (
        "Uma conversa do Ask não pode ser transferida.",
        "An Ask conversation cannot be handed off.",
    ),
    (
        "Uma conversa do Ask não tem chat lateral.",
        "An Ask conversation has no side chat.",
    ),
    (
        "Uma conversa do Ask usa a cópia do projeto; ela não pode removê-la.",
        "An Ask conversation uses the project's checkout; it cannot remove it.",
    ),
    (
        "Uma conversa do Claude com mais de 200 mil tokens parada compacta uns dois minutos antes do cache do prompt expirar (1 hora na assinatura, 5 minutos com chave de API), assim ao voltar a conversa inteira não é reenviada. Aparece como um turno /compact; conversas adiadas ficam como estão.",
        "A Claude conversation over 200,000 tokens that sits idle compacts about two minutes before its prompt cache expires (1 hour on a subscription, 5 minutes with an API key), so coming back does not resend the whole conversation. It shows as a /compact turn; snoozed conversations are left alone.",
    ),
    (
        "Uma conversa em execução ou em espera não pode ser adiada.",
        "A conversation that is running or waiting cannot be snoozed.",
    ),
    (
        "Uma conversa está trabalhando nesta pasta. Puxe quando ela terminar.",
        "A conversation is working in this folder. Pull when it finishes.",
    ),
    (
        "Uma conversa parada pelo limite da IA envia \"Continue from where you left off.\" quando o limite libera. Dá para cancelar em cada aviso.",
        "A conversation stopped by the provider's limit sends \"Continue from where you left off.\" at the reset. Each notice can cancel it.",
    ),
    ("Uma conversa à parte.", "A separate conversation."),
    (
        "Uma equipe precisa de 1 a 3 tarefas.",
        "A team needs 1 to 3 tasks.",
    ),
    (
        "Uma imagem atrás dos painéis de conversa. Ela fica neste Mac.",
        "An image behind the conversation panels. It stays on this Mac.",
    ),
    (
        "Uma pasta sem Git trabalha sempre nela mesma.",
        "A folder without Git always works in place.",
    ),
    ("Uma predefinição de cores.", "A color preset."),
    (
        "Uma página que o agente abre ou navega aparece num player pequeno sobre a conversa. Ela fica oculta enquanto o painel do navegador mostra a sessão.",
        "A page an agent opens or navigates appears in a small player over the conversation. It stays hidden while the browser panel shows that session.",
    ),
    (
        "Uma sessão que acompanha o PR (pelo cartão Ambiente ou quando você pede ao agente) recebe um turno automático, quando está ociosa, se um check falhar, alguém revisar ou comentar o código ou o branch tiver conflitos com a base. O GitHub é lido no máximo uma vez por minuto por PR; o acompanhamento para após 5 atualizações sem um novo commit.",
        "A session that watches its PR (from the Environment card or when you ask its agent) gets an automatic turn, while idle, when a check fails, someone reviews or comments on the code, or the branch conflicts with its base. GitHub is read at most once a minute per PR; watching stops after 5 updates without a new commit.",
    ),
    (
        "Uma vez, na primeira abertura de cada versão.",
        "Once, on the first launch of each version.",
    ),
    ("Usar", "Use"),
    ("Usar Este Mac", "Use this Mac"),
    ("Usar a ferramenta {}", "Use the {} tool"),
    ("Usar esta conta", "Use this account"),
    ("Usar fonte do sistema na interface", "Use system UI font"),
    ("Usar o padrão do provedor", "Use provider default"),
    ("Usar shell padrão do sistema", "Use system default shell"),
    ("Usar uma skill", "Use a skill"),
    (
        "Use PDF, DOCX, XLSX, PPTX ou CSV. Converta antes os arquivos DOC/XLS/PPT antigos.",
        "Use PDF, DOCX, XLSX, PPTX or CSV. Convert legacy DOC/XLS/PPT files first.",
    ),
    (
        "Use PDF, DOCX, XLSX, PPTX ou CSV. Converta arquivos antigos DOC/XLS/PPT primeiro.",
        "Use PDF, DOCX, XLSX, PPTX or CSV. Convert older DOC/XLS/PPT files first.",
    ),
    (
        "Use o Sirus pelo iPhone. O trabalho continua rodando neste Mac; o telefone só controla.",
        "Use Sirus from your iPhone. The work still runs on this Mac; the phone only controls it.",
    ),
    (
        "Use um endereço http(s) localhost com porta.",
        "Use a localhost http(s) address with a port.",
    ),
    (
        "Use um nome curto que ainda não existe, nem Padrão nem Privado.",
        "Use a short name that is not already taken, Default or Private.",
    ),
    (
        "Use um nome de pasta com letras, números, hífens, pontos ou sublinhados.",
        "Use a folder name of letters, digits, dashes, dots or underscores.",
    ),
    (
        "Use um nome de projeto de 1 a 100 caracteres.",
        "Use a project name of 1–100 characters.",
    ),
    (
        "Use um nome imprimível único de até 255 bytes, sem separadores nem nomes reservados de espaço de trabalho.",
        "Use a single printable name up to 255 bytes, without separators or reserved workspace names.",
    ),
    (
        "Use um owner/repo do GitHub, um endereço https:// ou um endereço git@.",
        "Use a GitHub owner/repo, an https:// address or a git@ address.",
    ),
    ("Use um único emoji.", "Use a single emoji."),
    ("Use uma imagem PNG ou JPEG.", "Use a PNG or JPEG image."),
    (
        "Use ⌃, ⌥ ou ⌘ com Espaço ou uma letra.",
        "Use ⌃, ⌥ or ⌘ with Space or a letter.",
    ),
    (
        "Use ⌘ com uma letra ou número livre.",
        "Use ⌘ with a free letter or number.",
    ),
    ("Uso", "Usage"),
    ("Uso do {}", "{} usage"),
    ("Uso em 5 horas", "5-hour usage"),
    ("Uso incluído", "Included usage"),
    ("Uso mensal", "Monthly usage"),
    ("Uso semanal", "Weekly usage"),
    ("Usou", "Used"),
    ("Usou a skill", "Used skill"),
    ("Usuário ou e-mail", "Username or email"),
    (
        "Usuário: {}. Guardado no Chaveiro do macOS.",
        "Username: {}. Kept in the macOS Keychain.",
    ),
    (
        "Vale para código nas transcrições, diffs e no editor.",
        "Applies to code in transcripts, diffs and the editor.",
    ),
    ("Vale por {} · uso único", "Valid for {} · single use"),
    (
        "Valores salvos das células. Fórmulas não são recalculadas; gráficos e formatação não são exibidos.",
        "Saved cell values. Formulas are not recalculated; charts and formatting are omitted.",
    ),
    (
        "Variáveis: {session-name} {id}. O identificador mantém branches únicas.",
        "Variables: {session-name} {id}. The identifier keeps branches unique.",
    ),
    (
        "Velocidade do corredor do composer",
        "Composer runner speed",
    ),
    ("Ver boas-vindas de novo", "Show the welcome again"),
    ("Ver código-fonte", "Show source"),
    ("Ver página", "Show page"),
    ("Ver saída", "Show output"),
    ("Ver uso do provedor", "View provider usage"),
    ("Verde", "Green"),
    (
        "Verifica em segundo plano se Codex, Claude e outras CLIs têm versões mais novas.",
        "Check Codex, Claude and other provider CLIs for newer versions in the background.",
    ),
    ("Verificando o Simulador…", "Checking the Simulator…"),
    ("Verificando…", "Checking…"),
    ("Verificar CLIs", "Check CLIs"),
    ("Verificar atualizações das CLIs", "Check for CLI updates"),
    ("Verificar de novo", "Check again"),
    ("Versão", "Version"),
    ("Versão {}", "Version {}"),
    ("Versão {}.", "Version {}."),
    ("Vidro", "Glass"),
    ("Vidro fumê", "Smoked glass"),
    ("Visibilidade na sessão", "Session visibility"),
    ("Visibilidade no chat vazio", "Empty chat visibility"),
    ("Visualização", "View"),
    ("Você", "You"),
    (
        "Você está na versão mais recente ({}).",
        "You are on the latest version ({}).",
    ),
    (
        "Você pode recuperar pela Lixeira no Finder. Tudo que estiver dentro da pasta vai junto.",
        "You can restore it from the Trash in Finder. Anything inside a folder goes with it.",
    ),
    (
        "Volta a mensagem para o composer e apaga a resposta",
        "Takes the message back to the composer and drops the reply",
    ),
    ("Voltar", "Back"),
    ("Voltar ao app", "Back to app"),
    ("Voltar ao último projeto", "Back to the last project"),
    ("Voltou", "Went back"),
    (
        "Vários agentes trabalhando juntos",
        "Several agents working together",
    ),
    ("Workers, KV, R2 e D1", "Workers, KV, R2 and D1"),
    ("Workspace da sessão", "Session workspace"),
    ("Workspace padrão da sessão", "Default session workspace"),
    (
        "a conversa atingiu o limite de terminais",
        "session reached the terminal limit",
    ),
    (
        "a conversa já tem um agente em execução",
        "session already has a running agent",
    ),
    (
        "a entrada servers não é um objeto",
        "the servers entry is not an object",
    ),
    (
        "a execução foi cancelada antes de a inicialização ser concluída",
        "execution was cancelled before startup completed",
    ),
    (
        "a execução foi cancelada durante a captura do espaço de trabalho",
        "execution was cancelled during workspace capture",
    ),
    (
        "a limpeza do utilitário expirou",
        "utility cleanup timed out",
    ),
    ("a mensagem do commit está vazia", "commit message is empty"),
    (
        "a mensagem do commit excede o limite de 8 KiB",
        "commit message exceeds the 8 KiB limit",
    ),
    ("a mensagem está vazia", "prompt is empty"),
    (
        "a mensagem excede o limite de 64 KiB",
        "prompt exceeds the 64 KiB limit",
    ),
    (
        "a pasta deve ser um caminho absoluto",
        "the folder must be an absolute path",
    ),
    (
        "a pasta do projeto excede seus limites",
        "project folder exceeds its bounds",
    ),
    ("a pasta não existe", "the folder does not exist"),
    (
        "a pasta pai deve ser um caminho absoluto",
        "the parent folder must be an absolute path",
    ),
    ("a pasta pai não existe", "the parent folder does not exist"),
    (
        "a prévia do diff exige um arquivo",
        "diff preview requires a file",
    ),
    (
        "a prévia suporta somente arquivos regulares",
        "preview supports regular files only",
    ),
    (
        "a raiz da skill mudou durante a leitura",
        "skill root changed during read",
    ),
    (
        "a saída limitada do utilitário foi excedida",
        "bounded utility output exceeded",
    ),
    ("abaixo do ritmo", "below pace"),
    ("abrir um app exige um Mac", "opening an app needs a Mac"),
    ("acima do orçamento de hoje", "over today's budget"),
    ("acima do ritmo", "above pace"),
    ("agente", "agent"),
    ("agentes", "agents"),
    ("agora", "now"),
    (
        "aparência do projeto inválida",
        "invalid project appearance",
    ),
    (
        "aqui só abrem imagens PNG, JPEG, GIF e WebP",
        "only PNG, JPEG, GIF and WebP images open here",
    ),
    (
        "armazenamento de skills indisponível",
        "skill storage unavailable",
    ),
    ("arquivo", "file"),
    (
        "arquivo binário não pode ser visualizado como texto",
        "binary file cannot be previewed as text",
    ),
    (
        "arquivo do espaço de trabalho de outra conversa",
        "foreign workspace file",
    ),
    ("arquivo grande demais", "file too large"),
    ("arquivos", "files"),
    (
        "as pastas abertas da barra lateral excedem seus limites",
        "open sidebar folders exceed their bounds",
    ),
    (
        "as referências da barra lateral excedem seus limites",
        "sidebar references exceed their bounds",
    ),
    ("atual", "current"),
    ("banco de dados indisponível", "database unavailable"),
    ("binário", "binary"),
    ("bloqueado", "blocked"),
    ("branch ou commit inválido", "invalid branch or commit"),
    (
        "clientRequestId deve ser um UUID com hífens",
        "clientRequestId must be a hyphenated UUID",
    ),
    ("comando", "command"),
    ("comandos", "commands"),
    (
        "consulta de arquivo do espaço de trabalho inválida",
        "invalid workspace file query",
    ),
    ("consulta de busca inválida", "invalid search query"),
    (
        "conversa do agente da equipe não encontrada",
        "team worker session not found",
    ),
    ("conversa não encontrada", "session not found"),
    (
        "digite o caminho completo da pasta",
        "type the folder's full path",
    ),
    (
        "documento da skill fora das raízes do catálogo",
        "skill document outside catalog roots",
    ),
    ("e mais {} arquivos", "and {} more files"),
    ("editor desconhecido", "unknown editor"),
    ("escolha uma pasta pai", "choose a parent folder"),
    ("escopo MCP desconhecido", "unknown MCP scope"),
    ("esta conversa não tem equipe", "this session has no team"),
    (
        "este modelo está desativado em Configurações",
        "this model is disabled in Settings",
    ),
    (
        "este provedor está desativado em Configurações",
        "this provider is disabled in Settings",
    ),
    ("estes arquivos", "these files"),
    ("estimativa a preços de tabela", "estimate at list prices"),
    (
        "estimativa parcial, há modelos sem preço",
        "partial estimate, some models have no price",
    ),
    ("falha no git branch", "git branch failed"),
    ("falha no git fetch", "git fetch failed"),
    ("falhou", "failed"),
    ("feito", "done"),
    ("ferramenta", "tool"),
    ("ferramentas", "tools"),
    ("guia não encontrado", "guide not found"),
    ("https://exemplo.com", "https://example.com"),
    ("há {} dias", "{} days ago"),
    ("há {} h", "{} h ago"),
    ("há {} min", "{} min ago"),
    ("iPhone conectado", "iPhone connected"),
    ("id de terminal inválido", "invalid terminal id"),
    (
        "interrompa as conversas em execução antes de remover este projeto",
        "stop running sessions before removing this project",
    ),
    (
        "interrompa o agente antes de excluir esta conversa",
        "stop the agent before deleting this session",
    ),
    ("liberado", "allowed"),
    ("limite do diff", "diff limit"),
    ("listagem de worktrees inválida", "invalid worktree listing"),
    ("mcp_servers não é uma tabela", "mcp_servers is not a table"),
    ("nenhum projeto", "no project"),
    (
        "nenhum remoto origin está configurado",
        "no origin remote is configured",
    ),
    ("nenhum servidor com esse nome", "no server with that name"),
    ("nome de servidor inválido", "invalid server name"),
    ("nome do branch inválido", "invalid branch name"),
    (
        "não foi possível alocar um caminho de worktree único",
        "could not allocate a unique worktree path",
    ),
    ("não há diretório para abrir", "no directory to open"),
    ("não há pasta pai", "no parent folder"),
    (
        "não é possível abrir caminhos reservados",
        "cannot open reserved paths",
    ),
    (
        "não é possível abrir um pull request a partir de um HEAD destacado",
        "cannot open a pull request from a detached HEAD",
    ),
    (
        "não é possível alterar o provedor ou o modelo de uma conversa em execução",
        "cannot change a running session provider or model",
    ),
    (
        "não é possível enviar um HEAD destacado",
        "cannot push a detached HEAD",
    ),
    (
        "não é possível gravar caminhos reservados",
        "cannot write reserved paths",
    ),
    ("não é possível gravar no git", "cannot write to git"),
    (
        "não é possível inspecionar a configuração de filtros do Git com segurança",
        "cannot inspect Git filter configuration safely",
    ),
    (
        "não é possível inspecionar a configuração de rede do Git com segurança",
        "cannot inspect Git network configuration safely",
    ),
    (
        "não é possível inspecionar a configuração do Git com segurança",
        "cannot inspect Git configuration safely",
    ),
    (
        "não é possível inspecionar com segurança os processos do terminal próprios",
        "cannot inspect owned terminal processes safely",
    ),
    (
        "não é possível inspecionar os hooks do Git configurados com segurança",
        "cannot inspect configured Git hooks safely",
    ),
    ("não é possível ler os processos", "cannot read processes"),
    (
        "não é possível puxar para um HEAD destacado",
        "cannot pull into a detached HEAD",
    ),
    ("não é um TOML válido", "not valid TOML"),
    ("não é um arquivo de imagem", "not an image file"),
    ("não é um arquivo de vídeo", "not a video file"),
    ("não é um arquivo regular", "not a regular file"),
    ("não é um branch de movimentação", "not a move branch"),
    ("não é um endereço do GitHub", "not a GitHub address"),
    (
        "o app não conseguiu abrir esta pasta",
        "the app could not open this folder",
    ),
    (
        "o arquivo não é um objeto JSON",
        "the file is not a JSON object",
    ),
    (
        "o arquivo é grande demais para editar (limite de 2 MiB)",
        "file is too large to edit (2 MiB limit)",
    ),
    (
        "o arquivo é grande demais para visualizar (limite de 2 MiB)",
        "file is too large to preview (2 MiB limit)",
    ),
    ("o caminho deve ser absoluto", "path must be absolute"),
    ("o caminho do worktree está vazio", "worktree path is empty"),
    ("o caminho não é um diretório", "path is not a directory"),
    (
        "o caminho sai da raiz permitida do projeto ou do worktree",
        "path escapes the allowed project or worktree root",
    ),
    (
        "o conteúdo do arquivo excede o limite de 1 MiB",
        "file content exceeds the 1 MiB limit",
    ),
    (
        "o diretório vinculado do espaço de trabalho está indisponível",
        "linked workspace directory is unavailable",
    ),
    (
        "o documento da skill excede o limite de 64 KiB ou não é um arquivo regular",
        "skill document exceeds the 64 KiB limit or is not a regular file",
    ),
    (
        "o documento da skill não está em UTF-8",
        "skill document is not UTF-8",
    ),
    (
        "o editor grava apenas arquivos regulares",
        "editor writes regular files only",
    ),
    ("o editor não está instalado", "editor is not installed"),
    (
        "o espaço de trabalho está fora da raiz do Git",
        "workspace is outside Git root",
    ),
    (
        "o espaço de trabalho mudou durante a admissão do turno",
        "workspace changed during turn admission",
    ),
    (
        "o espaço de trabalho mudou durante a busca de arquivos",
        "workspace changed during file search",
    ),
    (
        "o espaço de trabalho mudou durante a busca de texto",
        "workspace changed during text search",
    ),
    (
        "o git fetch não terminou a tempo",
        "git fetch did not finish in time",
    ),
    (
        "o git pull não terminou em 30 segundos.",
        "git pull did not finish in 30 seconds.",
    ),
    (
        "o motor de aparelhos não respondeu.",
        "the device hub did not answer.",
    ),
    (
        "o nome do projeto deve ter de 1 a 200 caracteres imprimíveis",
        "project name must contain 1–200 printable characters",
    ),
    (
        "o origin não tem branch padrão",
        "origin has no default branch",
    ),
    (
        "o tamanho do documento da skill mudou",
        "skill document changed size",
    ),
    ("o terminal não está em execução", "terminal is not running"),
    (
        "o título da conversa deve ter de 1 a 200 caracteres",
        "session title must contain 1–200 characters",
    ),
    (
        "o título da conversa excede 200 caracteres",
        "session title exceeds 200 characters",
    ),
    (
        "o worktree contém alterações não commitadas ou não rastreadas; preserve-as antes de remover",
        "worktree contains uncommitted or untracked changes; preserve them before removal",
    ),
    ("ontem", "yesterday"),
    ("origem da skill indisponível", "skill source unavailable"),
    ("origem da skill não encontrada", "skill source not found"),
    ("origem de skill inválida", "invalid skill source"),
    (
        "os caminhos do worktree não estão em UTF-8",
        "worktree paths are not UTF-8",
    ),
    ("parcial", "partial"),
    (
        "pasta de skill portátil insegura",
        "unsafe portable skill folder",
    ),
    ("pastas de projeto demais", "too many project folders"),
    ("por {}", "by {}"),
    ("projeto não encontrado", "project not found"),
    ("recusado", "declined"),
    (
        "recusando caminho de diff absoluto ou que sai do diretório",
        "refusing absolute or traversing diff path",
    ),
    (
        "remover um worktree exclui essa cópia de trabalho. confirme explicitamente.",
        "removing a worktree deletes that working copy. confirm explicitly.",
    ),
    (
        "remover worktrees da equipe exclui essas cópias de trabalho. confirme explicitamente.",
        "removing team worktrees deletes those working copies. confirm explicitly.",
    ),
    (
        "remover worktrees restantes exclui essas pastas. confirme explicitamente.",
        "removing leftover worktrees deletes those folders. confirm explicitly.",
    ),
    ("renova em {}", "resets in {}"),
    (
        "responsável pela skill de outra conversa",
        "foreign skill owner",
    ),
    (
        "responsável pelo espaço de trabalho de outra conversa",
        "foreign workspace owner",
    ),
    ("resumido", "truncated"),
    (
        "revisão do turno não disponível",
        "turn review not available",
    ),
    ("ritmo uniforme em {}%", "even pace at {}%"),
    ("saída {}", "exit {}"),
    ("sem preço", "no price"),
    ("tarefa da equipe não encontrada", "team task not found"),
    ("texto-colado.txt", "pasted-text.txt"),
    ("trabalhando", "working"),
    (
        "worktrees isolados exigem um repositório Git",
        "isolated worktrees require a Git repository",
    ),
    (
        "{files} preparados para o commit.",
        "{files} staged for the commit.",
    ),
    ("{n} arquivos", "{n} files"),
    ("{n} conversas selecionadas", "{n} conversations selected"),
    (
        "{n} conversas serão apagadas para sempre.",
        "{n} conversations will be gone for good.",
    ),
    (
        "{n} conversas serão apagadas para sempre. As que estão trabalhando ficam.",
        "{n} conversations will be gone for good. The ones working stay.",
    ),
    ("{} agentes", "{} agents"),
    ("{} ainda não está pronto", "{} is not ready yet"),
    ("{} anexos", "{} attachments"),
    ("{} arquivos", "{} files"),
    ("{} arquivos alterados", "{} changed files"),
    (
        "{} arquivos podem ser desfeitos.",
        "{} files can be undone.",
    ),
    ("{} assumiu", "{} took over"),
    ("{} commit novo em {}", "{} new commit on {}"),
    ("{} commits novos em {}", "{} new commits on {}"),
    ("{} contas", "{} accounts"),
    ("{} de {}", "{} of {}"),
    ("{} de {} checks falharam", "{} of {} checks failed"),
    ("{} de {} checks passaram", "{} of {} checks passed"),
    ("{} de {} checks rodando", "{} of {} checks running"),
    ("{} de {} concluídos", "{} of {} done"),
    (
        "{} de {} skills ativas · Em todas as origens",
        "{} of {} skills active · From all sources",
    ),
    ("{} dias", "{} days"),
    ("{} e mais {} arquivos", "{} and {} more files"),
    ("{} em execução", "{} running"),
    ("{} em segundo plano", "{} in the background"),
    (
        "{} entradas anteriores não são mais guardadas",
        "{} earlier entries are no longer kept",
    ),
    (
        "{} está instalado e costuma usar {}",
        "{} is installed and usually uses {}",
    ),
    ("{} falharam", "{} failed"),
    ("{} ferramentas", "{} tools"),
    (
        "{} foi editado depois desta resposta, nos mesmos trechos. Nada será desfeito: reverta à mão pelo diff.",
        "{} was edited after this reply, in the same lines. Nothing is undone: revert by hand from the diff.",
    ),
    ("{} fps · pior quadro {} ms", "{} fps · worst frame {} ms"),
    ("{} mais", "{} more"),
    ("{} mantidas", "{} kept"),
    (
        "{} mudaram de novo depois e ficaram como estão",
        "{} changed again since then and stayed as they are",
    ),
    (
        "{} mudou de novo em uma resposta posterior desta sessão. Desfaça essa resposta primeiro, ou reverta à mão pelo diff.",
        "{} changed again in a later reply of this session. Undo that reply first, or revert by hand from the diff.",
    ),
    (
        "{} não reconhece mais este Mac. Pareie de novo em Configurações → Conexões.",
        "{} no longer recognises this Mac. Pair it again in Settings → Connections.",
    ),
    ("{} passos", "{} steps"),
    ("{} pedidos na fila", "{} queued requests"),
    ("{} quer usar {}", "{} wants to use {}"),
    (
        "{} repositórios indisponíveis.",
        "{} repositories unavailable.",
    ),
    ("{} restante", "{} left"),
    ("{} sessões", "{} sessions"),
    ("{} subagentes", "{} subagents"),
    ("{} trabalhando", "{} working"),
    ("{} trabalhando há {}m {}s", "{} working for {}m {}s"),
    ("{} trabalhando há {}s", "{} working for {}s"),
    ("{} trabalhou por {}m {}s", "{} worked for {}m {}s"),
    ("{} trabalhou por {}s", "{} worked for {}s"),
    ("{} usado", "{} used"),
    ("{} · Encontrada em: {}", "{} · Found in: {}"),
    ("{}% restante", "{}% left"),
    ("{}% usado", "{}% used"),
    (
        "{}, de qualquer app, ou o ícone do Sirus na barra de menus abre um campo para começar uma conversa sem abrir a janela. O atalho muda em Atalhos.",
        "{}, from any app, or the Sirus icon in the menu bar opens a field to start a conversation without opening the window. Change the shortcut in Shortcuts.",
    ),
    ("À direita", "On the right"),
    ("À esquerda", "On the left"),
    ("Âmbar", "Amber"),
    (
        "É rascunho: marque como pronto antes de mesclar.",
        "It is a draft: mark it ready before merging.",
    ),
    ("Ícone", "Icon"),
    ("Ícone de projeto desconhecido.", "Unknown project icon."),
    ("Ícone do Dock", "Dock icon"),
    ("Ícone do app", "App icon"),
    (
        "Ícone ou estilo do Astro desconhecido.",
        "Unknown Astro icon or style.",
    ),
    ("Últimos 30 dias", "Last 30 days"),
    ("Últimos 7 dias", "Last 7 days"),
    ("últimas 24 horas", "last 24 hours"),
    ("últimos {} dias", "last {} days"),
    ("… {} linhas a mais", "… {} more lines"),
    (
        "⌥Espaço, de qualquer app, ou o ícone do Sirus na barra de menus abre um campo para começar uma conversa sem abrir a janela.",
        "⌥Space from any app, or the Sirus icon in the menu bar, opens a field to start a conversation without opening the window.",
    ),
    (
        "⌥Tab próximo · ⌥⇧Tab anterior · solte ⌥ para abrir",
        "⌥Tab next · ⌥⇧Tab previous · release ⌥ to open",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_has_no_duplicate_keys() {
        let mut seen = HashSet::new();
        for (portuguese, _) in ENTRIES {
            assert!(seen.insert(*portuguese), "duplicate key: {portuguese}");
        }
    }

    #[test]
    fn table_is_sorted_and_every_entry_translates() {
        for pair in ENTRIES.windows(2) {
            assert!(pair[0].0 < pair[1].0, "out of order: {}", pair[1].0);
        }
        for (portuguese, english) in ENTRIES {
            assert!(
                !english.is_empty() && english != portuguese,
                "no translation: {portuguese}"
            );
        }
    }

    #[test]
    fn a_host_message_reads_in_portuguese_when_known() {
        set_locale("pt-BR");
        assert_eq!(
            from_host("This reply cannot take instructions now."),
            "Esta resposta não aceita instruções agora."
        );
        assert_eq!(from_host("Something else."), "Something else.");
    }

    #[test]
    fn placeholders_match_between_languages() {
        let count = |text: &str| text.matches('{').count();
        for (portuguese, english) in ENTRIES {
            assert_eq!(
                count(portuguese),
                count(english),
                "placeholders differ: {portuguese}"
            );
        }
    }

    #[test]
    fn locale_switches_text_and_fills_templates() {
        // One test owns the global locale, so parallel tests never see it flip.
        set_locale("pt-BR");
        assert_eq!(t("Cancelar"), "Cancelar");
        assert_eq!(t("sem entrada nenhuma"), "sem entrada nenhuma");
        set_locale("en");
        assert_eq!(t("Cancelar"), "Cancel");
        assert_eq!(t("sem entrada nenhuma"), "sem entrada nenhuma");
        assert_eq!(code(), "en");
        assert_eq!(tf("{} de {}", &[&1, &3]), "1 of 3");
        set_locale("pt-BR");
        assert_eq!(tf("{} de {}", &[&1, &3]), "1 de 3");
        assert_eq!(code(), "pt-BR");
    }

    /// Lists the Portuguese literals (with accents) not passed to `t`/`tf`. Run with
    /// `cargo test i18n::tests::unwrapped -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn unwrapped() {
        let accents = "áàâãéêíóôõúçÁÀÂÃÉÊÍÓÔÕÚÇ";
        let mut count = 0;
        for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || name == "i18n.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for (number, line) in source.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let mut from = 0;
                while let Some(open) = line[from..].find('"').map(|i| i + from) {
                    let mut end = open + 1;
                    let bytes = line.as_bytes();
                    while end < line.len() && bytes[end] != b'"' {
                        end += if bytes[end] == b'\\' { 2 } else { 1 };
                    }
                    let end = end.min(line.len());
                    let text = &line[open + 1..end.min(line.len())];
                    let before = &line[..open];
                    if text.chars().any(|c| accents.contains(c))
                        && !before.ends_with("t(")
                        && !before.ends_with("tf(")
                    {
                        println!("{name}:{}: {text}", number + 1);
                        count += 1;
                    }
                    from = (end + 1).min(line.len());
                    if from >= line.len() {
                        break;
                    }
                }
            }
        }
        println!("{count} Portuguese literals not wrapped");
    }
}
