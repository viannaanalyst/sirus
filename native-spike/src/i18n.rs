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
    ENGLISH.load(Ordering::Relaxed)
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
    (
        "\"{}\" é encerrado. A conversa continua.",
        "\"{}\" is ended. The conversation goes on.",
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
    (
        "100% é um núcleo da CPU, como no Monitor de Atividade.",
        "100% is one CPU core, as in Activity Monitor.",
    ),
    ("5 horas · modelo reserva", "5-hour · reserve model"),
    (
        "60 fps fica mais suave e gera arquivos maiores. A página é capturada o mais rápido que ela permitir.",
        "60 fps is smoother and makes larger files. The page is captured as fast as it allows.",
    ),
    ("> comandos", "> commands"),
    (
        "A Tailscale não está conectada",
        "Tailscale is not connected",
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
        "A conexão com o computador caiu.",
        "The connection to the computer dropped.",
    ),
    (
        "A conexão com o motor caiu: {}",
        "The connection to the engine dropped: {}",
    ),
    ("A conversa continua.", "The conversation continues."),
    (
        "A fila aceita até 8 pedidos de 64 KiB cada. Confira também se a troca de modelo terminou.",
        "The queue holds up to 8 requests of 64 KiB each. Also check that any model change has finished.",
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
    ("A instalação falhou: {}", "Install failed: {}"),
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
        "A personalização do Dock não está disponível neste host.",
        "Dock customization is not available on this host.",
    ),
    ("A página diz", "The page says"),
    ("A página pede confirmação", "The page asks you to confirm"),
    ("A página pede um valor", "The page asks for a value"),
    (
        "A senha é removida do Chaveiro. Isso não pode ser desfeito.",
        "The password is removed from the Keychain. This cannot be undone.",
    ),
    ("A tela não conectou: {}", "The screen did not connect: {}"),
    (
        "A versão nova é baixada e instalada e espera você reiniciar. Se você não reiniciar, ela entra na próxima vez que abrir o app.",
        "A new version is downloaded and installed, then waits for you to restart. If you don't, it starts the next time you open the app.",
    ),
    ("A versão {} está disponível.", "Version {} is available."),
    (
        "A versão {} está instalada. Ela começa quando você reiniciar ou abrir o app de novo.",
        "Version {} is installed. It starts when you restart or open the app again.",
    ),
    ("Abaixo", "Below"),
    ("Abas de conversa", "Conversation tabs"),
    ("Aberto", "Open"),
    ("Abertos", "Open"),
    (
        "Abra o endereço no navegador de outro aparelho da sua Tailscale e digite o código.",
        "Open the address in a browser on another device of your tailnet and type the code.",
    ),
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
    ("Abrindo o chat lateral…", "Opening the side chat…"),
    ("Abrir", "Open"),
    ("Abrir Ajustes do Sistema", "Open System Settings"),
    ("Abrir Alterações", "Open Changes"),
    ("Abrir Arquivos", "Open Files"),
    ("Abrir Chat lateral", "Open Side chat"),
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
    ("Abrir página", "Open page"),
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
    ("Adicionar conector personalizado", "Add a custom connector"),
    ("Adicionar conta", "Add account"),
    ("Adicionar login", "Add login"),
    ("Adicionar perfil", "Add profile"),
    ("Adicionar personalizado", "Add custom"),
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
        "Adicione uma pasta com SKILL.md no diretório acima ou na pasta de skills de um provedor e atualize.",
        "Add a folder with SKILL.md to the directory above or to a provider's skills folder, then refresh.",
    ),
    ("Age sem perguntar.", "Acts without asking."),
    ("Agente", "Agent"),
    ("Agentes", "Agents"),
    ("Agentes neste Mac", "Agents on this Mac"),
    ("Agentes podem usar este login", "Agents may use this login"),
    ("Agora não", "Not now"),
    ("Aguardando o navegador…", "Waiting for the browser…"),
    (
        "Ainda não há o que compactar: esta conversa não começou.",
        "There is nothing to compact yet: this conversation has not started.",
    ),
    (
        "Ajusta o espaçamento sem mudar o tamanho do texto.",
        "Adjusts spacing without changing the text size.",
    ),
    ("Ajustes do projeto", "Project settings"),
    ("Alguém", "Someone"),
    ("Alterar arquivos", "Change files"),
    ("Alterações", "Changes"),
    ("Alterações desfeitas", "Changes undone"),
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
        "Aplicada ao enviar e mantida nos seus próximos envios manuais.",
        "Applied when you send, then retained for your next manual turns.",
    ),
    ("Aplicativo", "App"),
    ("App do iPhone", "iPhone app"),
    ("Aprova o que for seguro.", "Approves what is safe."),
    ("Aprovar", "Approve"),
    ("Aprovar por mim", "Auto-review"),
    ("Aprovação por app", "Approve each app"),
    ("Aprovou", "Approved"),
    ("Areia", "Sand"),
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
    (
        "Arraste para escolher a sua cor.",
        "Drag to pick your color.",
    ),
    (
        "As opções desta página voltam ao padrão do app.",
        "This page's options go back to the app's defaults.",
    ),
    (
        "As regras do repositório impedem a mesclagem.",
        "The repository rules block the merge.",
    ),
    ("Atalho", "Shortcut"),
    ("Atalho global de captura", "Global snap shortcut"),
    ("Atalhos", "Keybindings"),
    ("Ativa", "Active"),
    ("Ativar HTTPS", "Turn on HTTPS"),
    ("Ativar modo debug", "Turn debug mode on"),
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
        "Aumenta ou diminui tudo.",
        "Makes everything larger or smaller.",
    ),
    ("Aumentar zoom", "Zoom in"),
    ("Autocorreção", "Autocorrect"),
    ("Automático", "Automatic"),
    ("Avançado", "Advanced"),
    ("Avançar", "Forward"),
    ("Avançou", "Went forward"),
    ("Avisar com o app em foco", "Notify while app is in focus"),
    ("Avisos de atividade", "Activity alerts"),
    ("Avisos no app", "Activity toasts"),
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
    ("Branco", "White"),
    ("Brasa", "Ember"),
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
    (
        "Buscar conversas, mensagens ou comandos",
        "Search chats, messages or commands",
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
    ("Caminho", "Path"),
    (
        "Caminho do executável. Vazio usa o detectado.",
        "Executable path. Empty uses the detected one.",
    ),
    ("Cancelar", "Cancel"),
    ("Cancelar geração do título", "Cancel title generation"),
    ("Captura", "Screenshot"),
    ("Captura da tela", "Screenshot"),
    ("Captura de janela", "Window snap"),
    ("Captura salva em {}", "Screenshot saved to {}"),
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
    ("Celular ou outro computador", "Phone or another computer"),
    ("Chat lateral", "Side chat"),
    ("Chave conectada · {}", "Key connected · {}"),
    ("Checkout atual", "Current checkout"),
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
    ("Clique para entrar", "Click to sign in"),
    ("Colar token", "Paste token"),
    (
        "Cole um token do serviço; ele fica no Keychain, nunca nos ajustes.",
        "Paste a token from the service; it is kept in the Keychain, never in settings.",
    ),
    ("Comandos", "Commands"),
    ("Comece uma nova página", "Start a new page"),
    ("Comentar", "Comment"),
    ("Comentou", "Commented"),
    ("Comentário enviado.", "Comment sent."),
    ("Começar", "Get started"),
    ("Commit e Push", "Commit and Push"),
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
    ("Computador", "Computer"),
    ("Computador pareado.", "Computer paired."),
    ("Computadores", "Computers"),
    (
        "Conclua o login no navegador.",
        "Complete sign-in in your browser.",
    ),
    ("Concluir", "Done"),
    ("Concluída", "Done"),
    ("Concluídas", "Done"),
    ("Conectado", "Connected"),
    ("Conectado como {}", "Signed in as {}"),
    ("Conectados", "Connected"),
    ("Conectando a {}…", "Connecting to {}…"),
    ("Conectando à tela…", "Connecting to the screen…"),
    ("Conectando…", "Connecting…"),
    ("Conectar", "Connect"),
    ("Conectar a este Mac?", "Connect to this Mac?"),
    ("Conectar outro", "Connect another"),
    ("Conectar um aparelho", "Connect a device"),
    ("Conectar {}", "Connect {}"),
    (
        "Conecte Supabase, Linear, Notion e outros serviços uma vez; Claude, Codex, OpenCode e os outros agentes ACP passam a usá-los.",
        "Connect Supabase, Linear, Notion and other services once; Claude, Codex, OpenCode and the other ACP agents then use them.",
    ),
    ("Conectores", "Connectors"),
    ("Conexões", "Connections"),
    ("Configurações", "Settings"),
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
    ("Confortável", "Comfortable"),
    ("Conjunto da equipe", "Team pool"),
    (
        "Consultar Postgres, gerenciar projetos, funções e storage",
        "Query Postgres, manage projects, functions and storage",
    ),
    ("Consumo na barra lateral", "Usage in the sidebar"),
    ("Conta", "Account"),
    ("Conta padrão", "Default account"),
    ("Contas do GitHub", "GitHub accounts"),
    ("Contas do {}", "{} accounts"),
    ("Contexto compactado", "Context compacted"),
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
    ("Cor de destaque", "Accent color"),
    ("Cores", "Colors"),
    ("Cores do diff", "Diff colors"),
    ("Cores do tema", "Theme colors"),
    ("Corrigir CI automaticamente", "Fix CI automatically"),
    ("Criando…", "Creating…"),
    ("Criar", "Create"),
    ("Criar PR", "Create PR"),
    ("Criar branch…", "Create branch…"),
    ("Criar merge commit", "Create a merge commit"),
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
    ("Desativar", "Turn off"),
    ("Descartada", "Dismissed"),
    ("Descartar", "Discard"),
    ("Descartar alterações?", "Discard changes?"),
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
    ("Despreparar", "Unstage"),
    ("Detectado: {}", "Detected: {}"),
    ("Detectar novamente", "Detect again"),
    ("Diagnóstico", "Diagnostics"),
    ("Diagnóstico das CLIs", "CLI diagnostics"),
    ("Digite ao menos 3 letras.", "Type at least 3 letters."),
    (
        "Digite no navegador da Tailscale ou no app do Sirus. Ele vale por pouco tempo.",
        "Enter it in the Tailscale browser or in the Sirus app. It is valid for a short time.",
    ),
    ("Digite o código", "Type the code"),
    (
        "Digite o valor e aperte Enter",
        "Type the value and press Enter",
    ),
    ("Digite um endereço", "Type an address"),
    ("Digitou", "Typed"),
    ("Diminuir zoom", "Zoom out"),
    ("Diretório automático", "Automatic directory"),
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
    ("Dê um nome ao aparelho", "Give the device a name"),
    (
        "Dê um nome para reconhecer este aparelho na lista.",
        "Give it a name to tell it apart in the list.",
    ),
    ("Economia do cache", "Cache savings"),
    ("Editar", "Edit"),
    ("Editou", "Edited"),
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
    ("Endereço seguro (HTTPS)", "Secure address (HTTPS)"),
    ("Entendi", "Got it"),
    ("Enter durante o ditado", "Enter while dictating"),
    ("Entrada", "Input"),
    ("Entrada em cache", "Cached input"),
    (
        "Entrada indisponível por enquanto.",
        "Input is not available yet.",
    ),
    ("Entrada sem cache", "Uncached input"),
    ("Entrar", "Sign in"),
    ("Entrar de novo", "Sign in again"),
    (
        "Entrar só pelo app Mac: o login abre o navegador.",
        "Sign-in is only available in the Mac app: it opens the browser.",
    ),
    (
        "Entre no GitHub CLI em Configurações → Git.",
        "Sign in to the GitHub CLI in Settings → Git.",
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
    ("Enviar para segundo plano", "Send to background"),
    ("Enviar {} para o remoto?", "Push {} to the remote?"),
    (
        "Envie /compact sozinho, sem anexos e sem os modos equipe ou depuração.",
        "Send /compact on its own, without attachments, team or debug mode.",
    ),
    ("Equipe", "Team"),
    ("Esc fechar", "Esc close"),
    (
        "Escolha como o Sirus avisa quando o trabalho termina ou precisa da sua atenção.",
        "Choose how Sirus alerts you when work finishes or needs your attention.",
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
        "Escolha se uma mensagem enviada enquanto o agente trabalha espera na fila ou instrui a resposta atual (Codex e Claude). ⌘Enter faz o contrário só naquela mensagem.",
        "Choose whether a message sent while the agent works waits in the queue or steers the current reply (Codex and Claude). ⌘Enter does the opposite for one message.",
    ),
    ("Escolha um aparelho", "Choose a device"),
    ("Escolha um item", "Pick an item"),
    ("Escolher imagem", "Choose image"),
    ("Escolher outra pasta…", "Choose another folder…"),
    ("Escolher pasta", "Choose folder"),
    ("Escolher uma imagem", "Choose an image"),
    ("Escolher uma pasta…", "Choose a folder…"),
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
    ("Escrita de cache", "Cache write"),
    ("Escuro", "Dark"),
    ("Esforço e velocidade", "Effort and speed"),
    (
        "Esforço gerenciado pelo provedor; esta CLI/modelo não expõe ajuste manual.",
        "Provider-managed effort; this CLI/model exposes no manual adjustment.",
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
        "Esse computador não reconhece mais este Mac.",
        "That computer no longer recognises this Mac.",
    ),
    (
        "Esse computador perdeu o pareamento: pareie de novo.",
        "This computer lost its pairing: pair it again.",
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
        "Esta conversa parou por um erro",
        "This conversation stopped on an error",
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
    ("Estado atualizado.", "Status updated."),
    ("Este Mac", "This Mac"),
    (
        "Este PDF excede os limites de páginas ou imagens do leitor.",
        "This PDF exceeds the reader's page or image limits.",
    ),
    (
        "Este anexo não está mais disponível. Anexe novamente.",
        "This attachment is no longer available. Attach it again.",
    ),
    (
        "Este arquivo excede os limites de conteúdo do leitor. Use um documento menor.",
        "This file exceeds the reader's content limits. Use a smaller document.",
    ),
    ("Este código expirou", "This code expired"),
    (
        "Este diff excedeu o limite de armazenamento da revisão.",
        "This diff exceeded the review storage limit.",
    ),
    (
        "Este documento não tem conteúdo legível.",
        "This document has no readable content.",
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
        "Este pedido já tentou iniciar. Para mudar o texto, cancele e adicione outro pedido.",
        "This request already attempted to start. To change its text, cancel it and add another request.",
    ),
    (
        "Este projeto não tem um remote do GitHub (origin): publique-o no GitHub antes de mover.",
        "This project has no GitHub remote (origin): publish it to GitHub before moving.",
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
        "Este servidor não precisa de login.",
        "This server needs no sign-in.",
    ),
    ("Estilo da barra lateral", "Sidebar style"),
    ("Estilo do cursor", "Cursor style"),
    ("Estrutura", "Outline"),
    ("Eventos e sons", "Events and sounds"),
    ("Excluir", "Delete"),
    ("Excluir conversa?", "Delete conversation?"),
    ("Excluir o login de {}?", "Delete the login for {}?"),
    ("Excluir perfil", "Delete profile"),
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
    ("Exportar conversa", "Export conversation"),
    ("Extra alto", "Extra high"),
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
    ("Ferramenta", "Tool"),
    ("Ferramentas de desenvolvimento", "Developer tools"),
    ("Fila", "Queue"),
    ("Fixar", "Pin"),
    ("Fixar aba", "Pin tab"),
    ("Fixar ou desafixar a aba", "Pin or unpin the tab"),
    ("Fixo", "Fixed"),
    ("Floresta", "Forest"),
    (
        "Fluxos reutilizáveis encontrados nos provedores e no workspace atual.",
        "Reusable workflows found in providers and the current workspace.",
    ),
    ("Fonte da interface", "UI font"),
    ("Fonte de código", "Code font"),
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
    (
        "Funciona de qualquer lugar pelo endereço público deste Mac, sem Tailscale",
        "Works from anywhere through this Mac's public address, no Tailscale needed",
    ),
    ("Funcionalidades experimentais", "Experimental features"),
    ("Fundo do chat", "Chat background"),
    ("Geral", "General"),
    ("Gerando um código…", "Making a code…"),
    ("Gerar código", "Generate code"),
    ("Gerar título do commit", "Generate commit title"),
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
    ("Gravação de Tela", "Screen Recording"),
    (
        "Gravação salva em Filmes › Sirus",
        "Recording saved in Movies › Sirus",
    ),
    ("Gravações do navegador", "Browser recordings"),
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
        "Há mais arquivos do que os listados.",
        "There are more files than listed.",
    ),
    (
        "Há um comando rodando em \"{}\". Fechar encerra o shell e o que estiver rodando nele.",
        "A command is running in \"{}\". Closing ends the shell and whatever runs in it.",
    ),
    ("Idioma", "Language"),
    ("Ignorado", "Skipped"),
    ("Imagem atual", "Current image"),
    ("Imagem de fundo", "Background image"),
    ("Indisponível", "Unavailable"),
    ("Informado pelo provedor", "Reported by the provider"),
    ("Informar", "Provide"),
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
        "Instale a Tailscale neste Mac e no celular e entre com a mesma conta. É grátis para uso pessoal.",
        "Install Tailscale on this Mac and on your phone and sign in with the same account. It is free for personal use.",
    ),
    (
        "Instale o GitHub CLI (gh) para listar pull requests.",
        "Install the GitHub CLI (gh) to list pull requests.",
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
        "Investigar erros, issues e traces",
        "Investigate errors, issues and traces",
    ),
    ("Início", "Home"),
    ("Isso não pode ser desfeito.", "This can't be undone."),
    (
        "Issues do Jira e páginas do Confluence",
        "Jira issues and Confluence pages",
    ),
    (
        "Issues, pull requests e repositórios (token)",
        "Issues, pull requests and repositories (token)",
    ),
    (
        "Já existe um branch com esse nome.",
        "A branch with this name already exists.",
    ),
    ("Já usado por {}", "Already used by {}"),
    (
        "Leia antes de enviar. Nada foi enviado.",
        "Read it before you send. Nothing was sent.",
    ),
    (
        "Leia com a câmera do iPhone ou digite o código no app do Sirus.",
        "Scan with the iPhone camera, or type the code in the Sirus app.",
    ),
    (
        "Leia o QR ou digite o código",
        "Scan the QR or type the code",
    ),
    ("Leitura de cache", "Cache read"),
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
    ("Limite de 5 horas", "5-hour limit"),
    ("Limite de painéis atingido", "Pane limit reached"),
    ("Limite do histórico", "Scrollback limit"),
    ("Limite individual", "Individual limit"),
    ("Limite mensal", "Monthly limit"),
    ("Limite semanal", "Weekly limit"),
    ("Limites", "Limits"),
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
    ("Mais {} não exibidos", "{} more not shown"),
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
    ("Mensagem do commit", "Commit message"),
    ("Mensagem durante a resposta", "Message during a reply"),
    ("Mensagem para este subagente", "Message for this subagent"),
    ("Mesclado", "Merged"),
    ("Mesclados", "Merged"),
    (
        "Mesclagem: arquivos alterados em relação ao primeiro pai.",
        "Merge: files changed against the first parent.",
    ),
    ("Mesclar", "Merge"),
    ("Mesclar {}?", "Merge {}?"),
    ("Meta", "Goal"),
    (
        "Mexa o mouse e clique no fundo",
        "Move the mouse and click the background",
    ),
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
        "Mostra um anel de consumo de até dois provedores no rodapé da barra lateral.",
        "Show a usage ring for up to two providers at the bottom of the sidebar.",
    ),
    (
        "Mostra um código de uso único para conectar o app do Sirus no iPhone ou um navegador da sua Tailscale.",
        "Show a one-time code to connect the Sirus iPhone app or a browser on your tailnet.",
    ),
    ("Mostrar / ocultar barra lateral", "Toggle Sidebar"),
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
    ("Mover para {}", "Move to {}"),
    ("Mover “{}” para a Lixeira?", "Move “{}” to the Trash?"),
    ("Máximo", "Maximum"),
    ("Médio", "Medium"),
    ("Mínimo", "Minimal"),
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
    ("Nada por aqui.", "Nothing here."),
    ("Nada sobrando", "Nothing left over"),
    ("Nas mensagens", "In messages"),
    ("Navegador", "Browser"),
    ("Navegador do Sirus", "Sirus browser"),
    ("Navegador do sistema", "System browser"),
    (
        "Necessária para capturar a janela de outro app.",
        "Needed to capture another app's window.",
    ),
    (
        "Necessário para instalar o Sirus como app no celular e receber alertas. A Tailscale fornece o certificado.",
        "Needed to install Sirus as an app on the phone and to receive alerts. Tailscale provides the certificate.",
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
    ("Nenhum elemento foi marcado.", "No element was marked."),
    ("Nenhum executável", "No executable"),
    (
        "Nenhum favorito disponível. Marque um modelo com a estrela.",
        "No available favorites. Mark a model with the star.",
    ),
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
    ("Nenhuma alteração.", "No changes."),
    ("Nenhuma atividade ainda.", "No activity yet."),
    ("Nenhuma ação ainda.", "No actions yet."),
    (
        "Nenhuma conta do GitHub conectada.",
        "No GitHub account signed in.",
    ),
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
    ("Nome", "Name"),
    (
        "Nome da conta, Enter para entrar",
        "Account name, Enter to sign in",
    ),
    ("Nome da pasta", "Folder name"),
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
    ("Notificações", "Notifications"),
    ("Notificações do sistema", "System notifications"),
    ("Nova aba", "New tab"),
    ("Nova aba privada", "New private tab"),
    ("Nova conversa", "New conversation"),
    (
        "Nova conversa em worktree",
        "New conversation in a worktree",
    ),
    ("Nova sessão", "New Session"),
    ("Novas abas abrem em", "New tabs open in"),
    ("Novas sessões", "New threads"),
    ("Novo", "New"),
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
    ("Não encontrado", "Not found"),
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
    ("Não foi possível abrir {}.", "Could not open {}."),
    ("Não foi possível anexar.", "Couldn't attach."),
    ("Não foi possível atualizar: {}", "Could not update: {}"),
    (
        "Não foi possível capturar a página.",
        "Could not capture the page.",
    ),
    (
        "Não foi possível executar o git: {}",
        "Could not run git: {}",
    ),
    (
        "Não foi possível ler este PDF. Ele pode estar protegido por senha ou danificado.",
        "Could not read this PDF. It may be encrypted or damaged.",
    ),
    (
        "Não foi possível ler o commit.",
        "Could not read the commit.",
    ),
    ("Não foi possível ler o diff.", "Could not read the diff."),
    ("Não foi possível ler {}.", "Could not read {}."),
    (
        "Não foi possível preparar a imagem anotada.",
        "Could not prepare the annotated image.",
    ),
    ("Não foi possível verificar", "Could not check"),
    ("Não instalado", "Not installed"),
    ("Não permitir", "Don't allow"),
    (
        "Não é um link sirus://new.",
        "It is not a sirus://new link.",
    ),
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
        "O HEAD do Git mudou desde esta resposta ({} para {}): houve commit, checkout, rebase ou pull. Nada será desfeito. Abra o diff e reverta à mão, ou use git revert no novo commit.",
        "Git HEAD moved since this reply ({} to {}): a commit, checkout, rebase or pull happened. Nothing is undone. Open the diff and revert it by hand, or use git revert on the new commit.",
    ),
    (
        "O Keychain só existe no macOS.",
        "The Keychain only exists on macOS.",
    ),
    (
        "O QR code do app aponta para este endereço.",
        "The app's QR code points to this address.",
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
    (
        "O Sirus deste iPhone vai controlá-lo.",
        "Sirus on this iPhone will control it.",
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
    (
        "O arquivo está danificado, protegido por senha ou usa um formato não suportado.",
        "This file is damaged, encrypted or uses an unsupported format.",
    ),
    (
        "O branch está desatualizado em relação ao base.",
        "The branch is behind the base.",
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
        "O conteúdo textual não mudou; o arquivo ou suas permissões foram alterados.",
        "The text content did not change; the file or its permissions changed.",
    ),
    (
        "O código de login expirou. Tente de novo.",
        "The sign-in code expired. Try again.",
    ),
    ("O código tem seis dígitos.", "The code has six digits."),
    ("O diff foi truncado.", "The diff was truncated."),
    (
        "O endereço HTTPS dele na Tailscale, como https://nome.rede.ts.net",
        "Its HTTPS address on Tailscale, such as https://name.network.ts.net",
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
        "O fetch automático está indisponível enquanto a configuração Git do projeto puder executar helpers ou divulgar credenciais. Faça o fetch manualmente pelo terminal.",
        "Automatic fetch is unavailable while project Git configuration can execute helpers or disclose credentials. Fetch manually in your terminal.",
    ),
    (
        "O fundo já muda enquanto você escolhe. Dá para trocar depois nas Configurações.",
        "The background already changes as you choose. You can change it later in Settings.",
    ),
    ("O git falhou.", "Git failed."),
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
        "O login de {} sai deste Mac. Conversas que usam esta conta impedem a remoção.",
        "{}'s sign-in leaves this Mac. Conversations using this account prevent removal.",
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
        "O número do pull request não é válido.",
        "Its pull request number is not valid.",
    ),
    ("O pareamento foi recusado.", "Pairing was declined."),
    ("O pareamento não respondeu.", "Pairing did not respond."),
    ("O projeto não é válido.", "Its project is not valid."),
    (
        "O prompt só tinha caracteres ocultos, então pode ser malicioso.",
        "Its prompt had only hidden characters, so it may be malicious.",
    ),
    (
        "O prompt tem mais de 8.000 caracteres.",
        "Its prompt is longer than 8,000 characters.",
    ),
    ("O que um aparelho pode fazer", "What a device can do"),
    ("O que vamos fazer em {}?", "What shall we do in {}?"),
    (
        "O repositório não está no formato dono/nome.",
        "Its repository is not owner/name.",
    ),
    (
        "O sirus-host não foi encontrado.",
        "sirus-host was not found.",
    ),
    (
        "O terminal usa sempre o shell configurado no sistema.",
        "The PTY always launches $SHELL. This cannot be changed.",
    ),
    (
        "O trabalho da conversa, inclusive o que ainda não foi commitado, vai para uma branch sirus/move-… no GitHub do projeto. {} baixa essa branch e continua a conversa lá, com o texto até aqui. Esta conversa fica marcada como movida.",
        "The conversation's work, including what is not committed yet, goes to a sirus/move-… branch on the project's GitHub. {} fetches that branch and continues the conversation there, with the text so far. This conversation is marked as moved.",
    ),
    (
        "O uso do computador só está disponível no macOS.",
        "Computer use is available on macOS only.",
    ),
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
        "Onde seus aparelhos encontram este Mac na Tailscale.",
        "Where your devices find this Mac on Tailscale.",
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
        "Os agentes deixam de recebê-lo e o login dele é esquecido.",
        "Agents stop receiving it and its sign-in is forgotten.",
    ),
    (
        "Os anexos devem somar até 40 MiB.",
        "Attachments must total at most 40 MiB.",
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
        "Os cookies, logins e dados dos sites são removidos deste Mac, e as abas dele fecham.",
        "Its cookies, sign-ins and site data are removed from this Mac, and its tabs close.",
    ),
    (
        "Os diffs antigos foram liberados. O resumo e a confirmação foram preservados.",
        "Older diffs were released. The summary and acknowledgment are preserved.",
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
        "Os seis dígitos de `sirus-host pair`, ou do QR code da página Conexões do outro Mac.",
        "The six digits from `sirus-host pair`, or from the other Mac's Connections page.",
    ),
    (
        "Ou diga ao agente o que fazer em vez disso (recusa a aprovação)…",
        "Or tell the agent what to do instead (declines the approval)…",
    ),
    ("Ou digite no app", "Or type in the app"),
    (
        "Outro documento está carregando. Tente novamente em instantes.",
        "Another document is loading. Try again in a moment.",
    ),
    ("Outro limite", "Other limit"),
    ("Outros", "Others"),
    ("Outros modelos", "Other models"),
    ("Ouvir", "Listen"),
    ("Pacotes", "Packages"),
    ("Padrão", "Default"),
    ("Padrão do nome da branch", "Branch naming pattern"),
    ("Padrões", "Defaults"),
    (
        "Pagamentos, catálogo e pedidos",
        "Payments, catalog and orders",
    ),
    ("Painel Ambiente", "Environment panel"),
    ("Painel lateral", "Side panel"),
    ("Palavra por palavra", "Token by token"),
    ("Paleta de comandos", "Command Palette"),
    ("Parada", "Stopped"),
    ("Parando…", "Stopping…"),
    ("Parar", "Stop"),
    ("Parar agente", "Stop Agent"),
    ("Parar conversa?", "Stop conversation?"),
    ("Parar e enviar", "Stop and send"),
    ("Parar e revogar tudo", "Stop and revoke all"),
    (
        "Parar e transcrever no composer, ou parar e enviar a mensagem assim que for transcrita.",
        "Stop and transcribe into the composer, or stop and send the message once it is transcribed.",
    ),
    ("Parar processo?", "Stop process?"),
    ("Parar script", "Stop script"),
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
        "Permita HTTPS na sua tailnet",
        "Allow HTTPS on your tailnet",
    ),
    (
        "Permite que agentes usem o navegador do app. Desligado, novos agentes não recebem as ferramentas dele e os que estão rodando são recusados; seu painel de navegador não muda.",
        "Allow agents to use the in-app browser. Off, new agents do not get its tools and running ones are refused; your own browser pane is unaffected.",
    ),
    ("Permitido", "Allowed"),
    ("Permitir", "Allow"),
    ("Permitir nesta sessão", "Allow for this session"),
    ("Permitir outros aparelhos", "Allow other devices"),
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
    ("Peça qualquer coisa", "Ask for anything"),
    ("Planejamento", "Planning"),
    ("Planejamento é só leitura", "Planning stays read-only"),
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
    (
        "Preencher o código de {}",
        "Fill the authenticator code for {}",
    ),
    ("Preencher {}", "Fill {}"),
    ("Preferências padrão", "Core defaults"),
    ("Preparado", "Staged"),
    ("Preparados", "Staged"),
    ("Preparar", "Stage"),
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
    ("Provedor padrão", "Default provider"),
    ("Provedores", "Providers"),
    ("Prévia em HTML", "HTML preview"),
    ("Próximo projeto", "Next project"),
    ("Pular", "Skip"),
    ("Pull request criado", "Pull request created"),
    ("Pull request mesclado.", "Pull request merged."),
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
    ("Rebase e mesclar", "Rebase and merge"),
    ("Recarregar sem cache", "Reload without cache"),
    ("Recarregou", "Reloaded"),
    ("Recentes", "Recent"),
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
    ("Responder", "Reply"),
    ("Resposta incompleta.", "Incomplete reply."),
    (
        "Resposta inesperada do GitHub.",
        "Unexpected reply from GitHub.",
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
    (
        "Rodando o script de preparação…",
        "Running the setup script…",
    ),
    (
        "Rodando o script de término…",
        "Running the on-finish script…",
    ),
    ("Rodar ao terminar", "Run on finish"),
    ("Rodar de novo", "Run again"),
    ("Rodar preparação", "Run setup"),
    ("Rode comandos no projeto.", "Run commands in the project."),
    ("Rode o app num aparelho.", "Run the app on a device."),
    ("Rolou", "Scrolled"),
    ("Rápida", "Fast"),
    ("Rótulo da conta", "Account label"),
    ("Sair", "Sign out"),
    (
        "Salva contagens limitadas do ciclo de execução localmente. Prompts, output, paths e credenciais são excluídos.",
        "Stores bounded lifecycle counts locally. Prompts, output, paths and credentials are excluded.",
    ),
    ("Salvar", "Save"),
    ("Salvar login", "Save login"),
    ("Salvar login de {}?", "Save login for {}?"),
    ("Salvar scripts", "Save scripts"),
    ("Salvar, agentes podem usar", "Save, agents may use"),
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
    (
        "Selecione um fluxo para adicionar /nome-da-skill ao rascunho.",
        "Select a workflow to add /skill-name to your draft.",
    ),
    (
        "Selecione um pull request ou issue na lista.",
        "Select a pull request or issue from the list.",
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
    ("Skills do projeto", "Project skills"),
    ("Skills dos agentes", "Agent skills"),
    ("Skills portáteis", "Portable skills"),
    ("Slide {} de {}", "Slide {} of {}"),
    ("Sobre", "About"),
    ("Solicitar aprovação", "Request approval"),
    ("Somente leitura", "Read-only"),
    ("Sons", "Sounds"),
    ("Sons de notificação", "Notification sounds"),
    ("Squash e mesclar", "Squash and merge"),
    ("Suave", "Smooth"),
    ("Suavização de fontes", "Font smoothing"),
    ("Subagente {}", "Subagent {}"),
    ("Sublinhado", "Underline"),
    (
        "Sublinhado de ortografia e correções automáticas do macOS enquanto você escreve uma mensagem, em todas as conversas.",
        "macOS spelling underlines and automatic corrections while you write a message, in every conversation.",
    ),
    ("Substituindo imagem…", "Replacing image…"),
    ("Substituir", "Replace"),
    ("Substituir original", "Replace original"),
    (
        "Só aparece nas listas do Sirus. Os agentes continuam usando o mesmo identificador.",
        "Only shown in Sirus lists. Agents keep using the same identifier.",
    ),
    (
        "Só aparelhos da sua Tailscale chegam a este endereço.",
        "Only devices on your tailnet reach this address.",
    ),
    (
        "Só aparelhos na sua rede Tailscale alcançam este Mac, e cada um precisa de um QR code desta página.",
        "Only devices on your Tailscale network can reach this Mac, and each one needs a QR code from this page.",
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
    (
        "Tailscale no iPhone, com a mesma conta deste Mac",
        "Tailscale on the iPhone, with the same account as this Mac",
    ),
    ("Tamanho da fonte da interface", "UI font size"),
    ("Tamanho da fonte de código", "Code font size"),
    ("Tamanho da fonte do terminal", "Terminal font size"),
    ("Tamanho da interface", "Interface size"),
    ("Tamanho personalizado", "Custom size"),
    ("Tamanho real", "Actual size"),
    (
        "Também respeita prefers-reduced-motion do sistema.",
        "Also follows the system's prefers-reduced-motion.",
    ),
    ("Tarefa", "Task"),
    ("Tarefa concluída", "Task completed"),
    ("Tarefas, projetos e metas", "Tasks, projects and goals"),
    (
        "Teclas digitadas na página aparecem embaixo do vídeo. Campos de senha nunca aparecem.",
        "Keys typed on the page appear under the video. Password fields never appear.",
    ),
    ("Tem uma pergunta", "Has a question"),
    ("Tema", "Theme"),
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
    ("Translucidez", "Translucency"),
    ("Trazer de volta ao painel", "Bring back to the pane"),
    ("Trocar", "Change"),
    ("Trocar de projeto", "Switch project"),
    ("Trocar modelo e esforço", "Change model and effort"),
    ("Trocar método", "Change method"),
    ("Trocar token", "Replace token"),
    ("Trocar…", "Switch…"),
    ("Trouxe para a frente", "Brought to front"),
    ("Tudo pronto", "All set"),
    ("Título do commit", "Commit title"),
    ("Título do pull request", "Pull request title"),
    ("Título escrito por {}", "Title written by {}"),
    (
        "Um aparelho conectado pode fazer o que esta janela faz: abrir sessões, enviar mensagens, aprovar ações, fazer commit e push. Seletores de arquivo, ditado e controle do computador ficam só no Mac. Remova o aparelho que não usar mais.",
        "A connected device can do what this window does: start sessions, send messages, approve actions, commit and push. Pickers, dictation and computer use stay on the Mac. Remove a device you no longer use.",
    ),
    (
        "Um círculo marca cada clique no vídeo.",
        "A ring marks each click in the video.",
    ),
    (
        "Um endereço localhost que o script serve, como http://localhost:5173.",
        "A localhost address the script serves, such as http://localhost:5173.",
    ),
    ("Um script já está rodando", "A script is already running"),
    (
        "Um toque suave quando um agente termina um turno ou precisa de você. Não toca para a conversa que você está vendo.",
        "A soft chime when an agent finishes a turn or needs you. Not played for the conversation you are viewing.",
    ),
    (
        "Uma conversa do Claude com mais de 200 mil tokens parada compacta uns dois minutos antes do cache do prompt expirar (1 hora na assinatura, 5 minutos com chave de API), assim ao voltar a conversa inteira não é reenviada. Aparece como um turno /compact; conversas adiadas ficam como estão.",
        "A Claude conversation over 200,000 tokens that sits idle compacts about two minutes before its prompt cache expires (1 hour on a subscription, 5 minutes with an API key), so coming back does not resend the whole conversation. It shows as a /compact turn; snoozed conversations are left alone.",
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
        "Uma imagem atrás dos painéis de conversa. Ela fica neste Mac.",
        "An image behind the conversation panels. It stays on this Mac.",
    ),
    (
        "Uma permissão única na sua conta da Tailscale. Abra a página, confirme e volte para verificar de novo.",
        "A one-time switch in your Tailscale account. Open the page, confirm, then come back and check again.",
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
        "Use PDF, DOCX, XLSX, PPTX ou CSV. Converta arquivos antigos DOC/XLS/PPT primeiro.",
        "Use PDF, DOCX, XLSX, PPTX or CSV. Convert older DOC/XLS/PPT files first.",
    ),
    (
        "Use o Sirus pelo celular ou por outro computador. O trabalho continua rodando neste Mac; o outro aparelho só controla, pela sua rede privada da Tailscale.",
        "Use Sirus from your phone or another computer. The work still runs on this Mac; the other device only drives it, over your private Tailscale network.",
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
    ("abaixo do ritmo", "below pace"),
    ("acima do orçamento de hoje", "over today's budget"),
    ("acima do ritmo", "above pace"),
    ("agente", "agent"),
    ("agentes", "agents"),
    ("agora", "now"),
    ("arquivo", "file"),
    ("arquivos", "files"),
    ("atual", "current"),
    ("binário", "binary"),
    ("bloqueado", "blocked"),
    ("comando", "command"),
    ("comandos", "commands"),
    ("e mais {} arquivos", "and {} more files"),
    ("estes arquivos", "these files"),
    ("estimativa a preços de tabela", "estimate at list prices"),
    (
        "estimativa parcial, há modelos sem preço",
        "partial estimate, some models have no price",
    ),
    ("falhou", "failed"),
    ("feito", "done"),
    ("ferramenta", "tool"),
    ("ferramentas", "tools"),
    ("https://exemplo.com", "https://example.com"),
    ("há {} dias", "{} days ago"),
    ("há {} h", "{} h ago"),
    ("há {} min", "{} min ago"),
    ("iPhone do trabalho", "Work iPhone"),
    ("iPhone pessoal", "Personal iPhone"),
    ("liberado", "allowed"),
    (
        "o motor de aparelhos não respondeu.",
        "the device hub did not answer.",
    ),
    ("ontem", "yesterday"),
    ("parcial", "partial"),
    ("por {}", "by {}"),
    ("recusado", "declined"),
    ("renova em {}", "resets in {}"),
    ("resumido", "truncated"),
    ("ritmo uniforme em {}%", "even pace at {}%"),
    ("saída {}", "exit {}"),
    ("sem preço", "no price"),
    ("trabalhando", "working"),
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
    ("{} conectado", "{} connected"),
    ("{} de {}", "{} of {}"),
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
    ("À direita", "On the right"),
    ("À esquerda", "On the left"),
    (
        "É rascunho: marque como pronto antes de mesclar.",
        "It is a draft: mark it ready before merging.",
    ),
    ("Ícone do Dock", "Dock icon"),
    ("Últimos 30 dias", "Last 30 days"),
    ("Últimos 7 dias", "Last 7 days"),
    ("últimas 24 horas", "last 24 hours"),
    ("últimos {} dias", "last {} days"),
    ("… {} linhas a mais", "… {} more lines"),
    ("↑ ↓ navegar", "↑ ↓ navigate"),
    ("↵ abrir", "↵ open"),
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
