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
        "\"{}\" sai da lista. A pasta e os arquivos continuam no disco.",
        "\"{}\" leaves the list. Its folder and files stay on disk.",
    ),
    (
        "\"{}\" será apagada para sempre.",
        "\"{}\" will be gone for good.",
    ),
    ("%d/%m/%Y", "%m/%d/%Y"),
    ("%d/%m/%Y %H:%M", "%m/%d/%Y %H:%M"),
    ("1 agente", "1 agent"),
    ("1 arquivo", "1 file"),
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
        "A conexão com o motor caiu: {}",
        "The connection to the engine dropped: {}",
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
        "A lista está incompleta: há mais arquivos do que o limite.",
        "The list is incomplete: there are more files than the limit.",
    ),
    (
        "A personalização do Dock não está disponível neste host.",
        "Dock customization is not available on this host.",
    ),
    (
        "A versão nova é baixada e instalada e espera você reiniciar. Se você não reiniciar, ela entra na próxima vez que abrir o app.",
        "A new version is downloaded and installed, then waits for you to restart. If you don't, it starts the next time you open the app.",
    ),
    ("Abas de conversa", "Conversation tabs"),
    ("Abra os arquivos do projeto.", "Open the project files."),
    ("Abra um app ou qualquer site.", "Open an app or any site."),
    (
        "Abra {} e digite o código. Esta tela termina quando o GitHub confirmar.",
        "Open {} and enter the code. This screen finishes when GitHub confirms.",
    ),
    (
        "Abre o painel Ambiente automaticamente. Quando desativado, ele fica fechado até você abrir. A última ação de abrir/fechar também atualiza esta preferência.",
        "Open the chat Environment panel automatically on normal threads. When off, the panel stays closed until you open it. Your last open/close also updates this preference.",
    ),
    ("Abrindo o chat lateral…", "Opening the side chat…"),
    ("Abrir", "Open"),
    ("Abrir Ajustes do Sistema", "Open System Settings"),
    ("Abrir Alterações", "Open Changes"),
    ("Abrir Arquivos", "Open Files"),
    ("Abrir Chat lateral", "Open Side chat"),
    ("Abrir Navegador", "Open Browser"),
    ("Abrir Simulador", "Open Simulator"),
    ("Abrir Terminal", "Open Terminal"),
    (
        "Abrir diretório de dados do Sirus Code",
        "Open Sirus Code data directory",
    ),
    ("Abrir links no", "Open links in"),
    ("Abrir no editor", "Open in the editor"),
    ("Abrir ou fechar a sidebar", "Open or close the sidebar"),
    ("Abrir ou fechar o chat lateral", "Toggle side chat"),
    (
        "Abrir ou fechar o painel lateral",
        "Open or close the side panel",
    ),
    ("Abrir pasta de logs", "Open logs folder"),
    ("Abrir por padrão", "Open by default"),
    ("Abrir projeto", "Open Project"),
    (
        "Abrir pull requests e issues",
        "Open pull requests and issues",
    ),
    ("Abrir página", "Open page"),
    ("Abrir terminal", "Open Terminal"),
    ("Abrir um painel", "Open a panel"),
    ("Acessibilidade", "Accessibility"),
    ("Acesso completo", "Full access"),
    ("Acesso dos agentes ao navegador", "Agent browser access"),
    ("Acesso remoto", "Remote access"),
    ("Acesso total", "Full access"),
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
    ("Adicionar conta", "Add account"),
    (
        "Adicione pastas com SKILL.md aqui. Skills invocadas funcionam entre provedores; a cópia do próprio provedor tem prioridade.",
        "Add folders with SKILL.md here. Invoked skills work across providers; the provider's own copy takes priority.",
    ),
    (
        "Adicione uma pasta com SKILL.md no diretório acima ou na pasta de skills de um provedor e atualize.",
        "Add a folder with SKILL.md to the directory above or to a provider's skills folder, then refresh.",
    ),
    ("Age sem perguntar.", "Acts without asking."),
    ("Agente", "Agent"),
    (
        "Ajusta o espaçamento sem mudar o tamanho do texto.",
        "Adjusts spacing without changing the text size.",
    ),
    ("Ajustes do projeto", "Project settings"),
    ("Alterar arquivos", "Change files"),
    ("Alterações", "Changes"),
    ("Alternar nível de raciocínio", "Cycle reasoning effort"),
    ("Alternar painel direito", "Toggle right panel"),
    ("Alto", "High"),
    ("Alto contraste", "High contrast"),
    ("Ambiente", "Environment"),
    ("Ameixa", "Plum"),
    ("Amostra", "Sample"),
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
    (
        "Ao abrir o Sirus Code e a cada poucas horas.",
        "When Sirus Code opens and every few hours.",
    ),
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
    ("Aplicativo", "App"),
    ("Aprova o que for seguro.", "Approves what is safe."),
    ("Aprovar", "Approve"),
    ("Aprovar por mim", "Auto-review"),
    ("Aprovação por app", "Approve each app"),
    ("Areia", "Sand"),
    (
        "Arquivo binário: não dá para mostrar o conteúdo aqui.",
        "Binary file: its content cannot be shown here.",
    ),
    ("Arquivo vazio.", "Empty file."),
    ("Arquivos", "Files"),
    ("Arquivos e pastas", "Files and folders"),
    (
        "Arraste para escolher a sua cor.",
        "Drag to pick your color.",
    ),
    ("Atalho", "Shortcut"),
    ("Atalho global de captura", "Global snap shortcut"),
    ("Atalhos", "Keybindings"),
    ("Ativa", "Active"),
    ("Ativar HTTPS", "Turn on HTTPS"),
    ("Ativar modo debug", "Turn debug mode on"),
    ("Atividade recente", "Recent activity"),
    ("Atualizado agora", "Updated now"),
    ("Atualizado há {} h", "Updated {} h ago"),
    ("Atualizado há {} min", "Updated {} min ago"),
    ("Atualizando…", "Updating…"),
    (
        "Atualizar o branch padrão automaticamente",
        "Automatically pull the default branch",
    ),
    (
        "Atualizar referências ao abrir projeto",
        "Automatically fetch when opening project",
    ),
    ("Atualizar skills", "Refresh skills"),
    ("Atualizações", "Updates"),
    (
        "Aumenta ou diminui tudo.",
        "Makes everything larger or smaller.",
    ),
    ("Autocorreção", "Autocorrect"),
    ("Automático", "Automatic"),
    ("Avançado", "Advanced"),
    ("Avançar", "Forward"),
    ("Avisar com o app em foco", "Notify while app is in focus"),
    ("Avisos de atividade", "Activity alerts"),
    ("Avisos no app", "Activity toasts"),
    ("Ações recentes", "Recent actions"),
    (
        "Baixar atualizações em segundo plano",
        "Download updates in the background",
    ),
    ("Baixo", "Low"),
    ("Bancos Prisma Postgres", "Prisma Postgres databases"),
    ("Barra", "Bar"),
    ("Barra de ícones", "Icon rail"),
    (
        "Barra de ícones, com as conversas nas abas, ou uma lista com todas as conversas.",
        "An icon rail, with conversations in the tabs, or a list with every conversation.",
    ),
    ("Barra lateral", "Sidebar"),
    ("Bloco", "Block"),
    ("Branco", "White"),
    ("Brasa", "Ember"),
    (
        "Busca o branch padrão do origin e baseia novos worktrees nele, e não no branch local. Sem origin ou sem rede, eles começam do branch local.",
        "Fetches origin's default branch and bases new worktrees on it instead of the local branch. Without origin or a network, they start from the local branch.",
    ),
    ("Buscar", "Search"),
    ("Buscar arquivos", "Search files"),
    (
        "Buscar arquivos pelo nome…  (> para comandos)",
        "Search files by name…  (> for commands)",
    ),
    ("Buscar configurações", "Search Settings"),
    (
        "Buscar conversas, mensagens ou comandos",
        "Search chats, messages or commands",
    ),
    ("Buscar em todas as conversas", "Search all conversations"),
    ("Buscar modelos...", "Search models..."),
    ("Buscar nesta conversa", "Find in conversation"),
    ("Buscar ou criar worktree…", "Search or create worktree…"),
    ("Buscar projetos…", "Search projects…"),
    (
        "Buscar, ler e editar páginas e bancos",
        "Search, read and edit pages and databases",
    ),
    (
        "Cada conversa fica na conta que a iniciou.",
        "Each conversation stays on the account that started it.",
    ),
    (
        "Cada pergunta leva o que a sessão principal está fazendo agora. Começa em só leitura e usa o mesmo workspace.",
        "Each question carries what the main session is doing right now. It starts read-only and uses the same workspace.",
    ),
    ("Cancelar", "Cancel"),
    ("Captura de janela", "Window snap"),
    ("Capturar uma janela aberta", "Capture an open app window"),
    (
        "Capturas da janela do app aprovado.",
        "Screenshots of the approved app's window.",
    ),
    ("Carregando catálogo de modelos…", "Loading model catalog…"),
    ("Carregando…", "Loading…"),
    ("Carregar mais", "Load more"),
    ("Carregar mensagens anteriores", "Load earlier messages"),
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
    ("Comandos", "Commands"),
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
    (
        "Compactar conversas paradas do Claude antes do cache expirar (beta)",
        "Compact idle Claude conversations before the cache expires (beta)",
    ),
    ("Compartilhadas (.agents)", "Shared (.agents)"),
    ("Componentes UI Arc", "UI Arc components"),
    ("Comportamento", "Behavior"),
    ("Comportamento do agente", "Agent behavior"),
    ("Comportamento do chat", "Chat behavior"),
    ("Computador", "Computer"),
    (
        "Conclua o login no navegador.",
        "Complete sign-in in your browser.",
    ),
    ("Concluídas", "Done"),
    ("Conectado", "Connected"),
    ("Conectados", "Connected"),
    ("Conectando…", "Connecting…"),
    ("Conectar", "Connect"),
    ("Conectar um aparelho", "Connect a device"),
    (
        "Conecte Supabase, Linear, Notion e outros serviços uma vez; Claude, Codex, OpenCode e os outros agentes ACP passam a usá-los.",
        "Connect Supabase, Linear, Notion and other services once; Claude, Codex, OpenCode and the other ACP agents then use them.",
    ),
    ("Conectores", "Connectors"),
    ("Conexões", "Connections"),
    ("Configurações", "Settings"),
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
    (
        "Continuar conversas interrompidas após uma falha",
        "Continue interrupted conversations after a crash",
    ),
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
    ("Copiar ID da conversa", "Copy conversation ID"),
    ("Copiar caminho", "Copy path"),
    ("Cor de destaque", "Accent color"),
    ("Cores", "Colors"),
    ("Cores do diff", "Diff colors"),
    ("Cores do tema", "Theme colors"),
    ("Corrigir CI automaticamente", "Fix CI automatically"),
    (
        "Criar, publicar e gerenciar sites",
        "Create, deploy and manage sites",
    ),
    ("Criação", "Created"),
    ("Código", "Code"),
    ("Código de confirmação", "Confirmation code"),
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
        "Deixe o Codex, o Claude Code e o OpenCode verem e usarem apps do Mac que você aprovar, em segundo plano.",
        "Let Codex, Claude Code and OpenCode see and use Mac apps you approve, in the background.",
    ),
    ("Densidade da interface", "UI density"),
    (
        "Depois de permitir nos Ajustes do Sistema, feche e reabra o Sirus Code se o status não atualizar.",
        "After allowing in System Settings, quit and reopen Sirus Code if the status does not update.",
    ),
    ("Depois do turno", "After the turn"),
    ("Desafixar", "Unpin"),
    ("Desafixar aba", "Unpin tab"),
    ("Desativar", "Turn off"),
    ("Designs e materiais da marca", "Designs and brand assets"),
    (
        "Designs, componentes e variáveis",
        "Designs, components and variables",
    ),
    ("Desligada", "Off"),
    ("Despreparar", "Unstage"),
    ("Detectar novamente", "Detect again"),
    ("Diagnóstico", "Diagnostics"),
    (
        "Digite no navegador da Tailscale ou no app do Sirus. Ele vale por pouco tempo.",
        "Enter it in the Tailscale browser or in the Sirus app. It is valid for a short time.",
    ),
    (
        "Digite o valor e aperte Enter",
        "Type the value and press Enter",
    ),
    ("Digite um endereço", "Type an address"),
    ("Diretório automático", "Automatic directory"),
    ("Diretório de trabalho", "Working directory behavior"),
    ("Diretório personalizado", "Custom directory"),
    ("Do tema", "From theme"),
    (
        "Documentação atualizada de bibliotecas",
        "Up-to-date library documentation",
    ),
    ("Editou", "Edited"),
    ("Efeito de luz nos botões", "Cursor-reactive button effects"),
    ("Efeito do fundo", "Background effect"),
    ("Efeitos e movimento", "Effects and motion"),
    ("Em breve", "Coming soon"),
    (
        "Em qualquer app, o atalho captura a janela da frente e anexa à conversa aberta no Sirus Code. Nada é enviado.",
        "From any app, the shortcut captures the window in front and attaches it to the conversation open in Sirus Code. Nothing is sent.",
    ),
    (
        "Encontrar, adicionar e concluir tarefas",
        "Find, add and complete tasks",
    ),
    (
        "Encontrar, criar e atualizar issues e projetos",
        "Find, create and update issues and projects",
    ),
    ("Endereço", "Address"),
    ("Endereço do QR code", "QR code address"),
    ("Endereço seguro (HTTPS)", "Secure address (HTTPS)"),
    ("Enter durante o ditado", "Enter while dictating"),
    ("Entrar", "Sign in"),
    ("Entrar de novo", "Sign in again"),
    (
        "Entrar só pelo app Mac: o login abre o navegador.",
        "Sign-in is only available in the Mac app: it opens the browser.",
    ),
    ("Enviando…", "Pushing…"),
    ("Enviar alerta de teste", "Send a test alert"),
    (
        "Enviar e começar nova conversa",
        "Send and start new thread",
    ),
    ("Equipe", "Team"),
    ("Esc fechar", "Esc close"),
    (
        "Escolha como o Sirus Code avisa quando o trabalho termina ou precisa da sua atenção.",
        "Choose how Sirus Code alerts you when work finishes or needs your attention.",
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
    ("Escolher imagem", "Choose image"),
    ("Escolher pasta", "Choose folder"),
    ("Escolher uma imagem", "Choose an image"),
    (
        "Escreva a meta e aperte Enter",
        "Write the goal and press Enter",
    ),
    ("Escuro", "Dark"),
    ("Esforço e velocidade", "Effort and speed"),
    (
        "Esforço gerenciado pelo provedor; esta CLI/modelo não expõe ajuste manual.",
        "Provider-managed effort; this CLI/model exposes no manual adjustment.",
    ),
    ("Espaçosa", "Spacious"),
    (
        "Essa conta do GitHub não está conectada.",
        "That GitHub account is not signed in.",
    ),
    ("Esta parte entra em seguida.", "This part comes next."),
    (
        "Esta pasta não é um repositório Git.",
        "This folder is not a Git repository.",
    ),
    (
        "Este provedor não informa uso.",
        "This provider does not report usage.",
    ),
    ("Estilo da barra lateral", "Sidebar style"),
    ("Estilo do cursor", "Cursor style"),
    ("Eventos e sons", "Events and sounds"),
    ("Excluir", "Delete"),
    ("Excluir conversa?", "Delete conversation?"),
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
    ("Extra alto", "Extra high"),
    (
        "Faturas, pedidos e pagamentos",
        "Invoices, orders and payments",
    ),
    ("Favoritos", "Favorites"),
    (
        "Fazer deploy e gerenciar serviços",
        "Deploy and manage services",
    ),
    ("Fechar aba", "Close tab"),
    ("Fechar abas à direita", "Close tabs to the right"),
    ("Fechar abas à esquerda", "Close tabs to the left"),
    ("Fechar outras abas", "Close other tabs"),
    ("Fechar todas as abas", "Close all tabs"),
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
    ("Funcionalidades experimentais", "Experimental features"),
    ("Fundo do chat", "Chat background"),
    ("Geral", "General"),
    ("Gerar código", "Generate code"),
    (
        "Gerenciadores de senha, segurança do sistema, terminais e ferramentas de automação nunca podem ser controlados.",
        "Password managers, system security, terminals and automation tools can never be controlled.",
    ),
    ("Git detectado", "Git detected"),
    ("Gravação de Tela", "Screen Recording"),
    ("Gravações do navegador", "Browser recordings"),
    ("Histórico · {}", "History · {}"),
    ("Hoje", "Today"),
    ("Hora de renovar já passou", "Renewal time has passed"),
    (
        "Há conflitos: resolva-os antes de preparar.",
        "There are conflicts: resolve them before staging.",
    ),
    ("Idioma", "Language"),
    ("Imagem atual", "Current image"),
    ("Imagem de fundo", "Background image"),
    ("Indisponível", "Unavailable"),
    ("Informar", "Provide"),
    ("Inicialização", "Startup"),
    (
        "Iniciar novos worktrees a partir do origin",
        "Start new worktrees from origin",
    ),
    (
        "Iniciar uma sessão nova quando o contexto estiver quase cheio",
        "Start a fresh session when the context is nearly full",
    ),
    ("Iniciou", "Started"),
    ("Instalado", "Installed"),
    ("Instalado · {}", "Installed · {}"),
    ("Instalação", "Installation"),
    (
        "Instale a Tailscale neste Mac e no celular e entre com a mesma conta. É grátis para uso pessoal.",
        "Install Tailscale on this Mac and on your phone and sign in with the same account. It is free for personal use.",
    ),
    ("Instruir", "Steer"),
    (
        "Integrações que disponibilizam modelos para sessões.",
        "Integrations that provide models for sessions.",
    ),
    (
        "Investigar erros, issues e traces",
        "Investigate errors, issues and traces",
    ),
    ("Isso não pode ser desfeito.", "This can't be undone."),
    (
        "Issues do Jira e páginas do Confluence",
        "Jira issues and Confluence pages",
    ),
    (
        "Issues, pull requests e repositórios (token)",
        "Issues, pull requests and repositories (token)",
    ),
    ("Lenta", "Slow"),
    (
        "Ler os controles dos apps e enviar cliques e teclas.",
        "Read app controls and send clicks and keys.",
    ),
    ("Leu", "Read"),
    ("Liberados agora", "Allowed now"),
    ("Liberar espaço", "Free up space"),
    ("Ligada", "On"),
    (
        "Ligado. Os novos QR codes abrem o endereço seguro; aparelhos pareados antes precisam de um novo código por lá.",
        "On. New QR codes open the secure address; devices paired before need a new code there.",
    ),
    ("Limite de 5 horas", "5-hour limit"),
    ("Limite do histórico", "Scrollback limit"),
    ("Limite individual", "Individual limit"),
    ("Limite mensal", "Monthly limit"),
    ("Limite semanal", "Weekly limit"),
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
    (
        "Lista as sessões em que um agente está trabalhando ao lado do campo de mensagem, quando duas ou mais rodam ao mesmo tempo.",
        "Lists the sessions an agent is working on beside the composer, once two or more run at the same time.",
    ),
    ("Lista de conversas", "Conversation list"),
    ("Local dos worktrees", "Worktree location"),
    ("Login expirado", "Sign-in expired"),
    ("Logo do projeto", "Project logo"),
    ("Logs de desenvolvimento", "Developer logs"),
    ("Luminosidade da cor", "Color lightness"),
    ("Mais antigas", "Older"),
    ("Mais escura ou mais clara.", "Darker or lighter."),
    ("Manter agentes prontos", "Keep agents ready"),
    (
        "Manter o Mac acordado para o celular",
        "Keep the Mac awake for the phone",
    ),
    (
        "Mantém a fonte escolhida guardada para quando esta opção estiver desligada.",
        "Keeps the chosen font saved for when this option is off.",
    ),
    ("Mascote", "Mascot"),
    ("Materiais", "Materials"),
    ("Matiz da cor", "Color hue"),
    ("Matiz do fundo", "Background hue"),
    ("Mensagem do commit", "Commit message"),
    ("Mensagem durante a resposta", "Message during a reply"),
    (
        "Mesclagem: arquivos alterados em relação ao primeiro pai.",
        "Merge: files changed against the first parent.",
    ),
    ("Meta", "Goal"),
    ("Modelos do Cursor", "Cursor models"),
    ("Modelos, datasets e Spaces", "Models, datasets and Spaces"),
    ("Modo debug", "Debug mode"),
    ("Modo planejamento", "Planning mode"),
    (
        "Mostra as conversas do projeto como abas no topo da janela. Desligado, a barra mostra só a conversa atual.",
        "Show the project's conversations as tabs at the top of the window. Off, the title bar shows only the current conversation.",
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
    ("Mostrar/ocultar Ambiente", "Toggle Environment"),
    (
        "Mostre notificações e toque sons também enquanto o Sirus Code estiver ativo.",
        "Also show system alerts and play sounds while Sirus Code is active.",
    ),
    (
        "Mostre um aviso para sessões que você não está visualizando.",
        "Show an in-app alert for sessions you are not currently viewing.",
    ),
    ("Mover", "Move"),
    ("Máximo", "Maximum"),
    ("Médio", "Medium"),
    ("Mínimo", "Minimal"),
    (
        "Na tomada, o Mac não entra em repouso e seus aparelhos sempre o alcançam. A tela continua apagando e bloqueando; na bateria, o repouso funciona normalmente. Fechar a tampa ainda põe o Mac em repouso.",
        "While plugged in, the Mac does not go to sleep, so your devices can always reach it. The screen still turns off and locks; on battery, sleep works as usual. Closing the lid still puts it to sleep.",
    ),
    ("Nada encontrado.", "Nothing matches."),
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
    ("Nenhum", "None"),
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
    ("Nenhum executável", "No executable"),
    (
        "Nenhum favorito disponível. Marque um modelo com a estrela.",
        "No available favorites. Mark a model with the star.",
    ),
    (
        "Nenhum login do GitHub está esperando.",
        "No GitHub sign-in is waiting.",
    ),
    (
        "Nenhum modelo corresponde à busca.",
        "No model matches the search.",
    ),
    (
        "Nenhum modelo habilitado disponível neste provedor.",
        "No enabled models available from this provider.",
    ),
    ("Nenhum para limpar", "None to clean"),
    ("Nenhum servidor local", "No local servers"),
    ("Nenhuma", "None"),
    ("Nenhuma alteração.", "No changes."),
    ("Nenhuma ação ainda.", "No actions yet."),
    (
        "Nenhuma conta do GitHub conectada.",
        "No GitHub account signed in.",
    ),
    ("Nenhuma pasta criada", "No folders created"),
    ("Nenhuma skill encontrada", "No skills found"),
    ("No Sirus Code", "In Sirus Code"),
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
    ("Notificações", "Notifications"),
    ("Notificações do sistema", "System notifications"),
    ("Nova conversa", "New conversation"),
    (
        "Nova conversa em worktree",
        "New conversation in a worktree",
    ),
    ("Nova sessão", "New Session"),
    ("Novas sessões", "New threads"),
    ("Novo projeto", "New project"),
    ("Novo worktree", "New worktree"),
    ("Não", "No"),
    (
        "Não consegui copiar os dados do Sirus.",
        "Could not copy the Sirus data.",
    ),
    (
        "Não consegui preparar a cópia dos dados.",
        "Could not prepare the data copy.",
    ),
    ("Não detectado", "Not detected"),
    ("Não foi possível abrir {}.", "Could not open {}."),
    (
        "Não foi possível executar o git: {}",
        "Could not run git: {}",
    ),
    (
        "Não foi possível ler o commit.",
        "Could not read the commit.",
    ),
    ("Não foi possível ler o diff.", "Could not read the diff."),
    ("Não instalado", "Not installed"),
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
        "O QR code do app aponta para este endereço.",
        "The app's QR code points to this address.",
    ),
    ("O agente está trabalhando…", "The agent is working…"),
    ("O agente fez uma pergunta.", "The agent asked a question."),
    (
        "O agente só vê e usa os apps que você liberar, naquela sessão. Nada fica guardado depois que você fecha o app.",
        "Agents only see and use apps you allow, for that session. Nothing is remembered after you quit.",
    ),
    (
        "O código de login expirou. Tente de novo.",
        "The sign-in code expired. Try again.",
    ),
    (
        "O fetch automático está indisponível enquanto a configuração Git do projeto puder executar helpers ou divulgar credenciais. Faça o fetch manualmente pelo terminal.",
        "Automatic fetch is unavailable while project Git configuration can execute helpers or disclose credentials. Fetch manually in your terminal.",
    ),
    ("O git falhou.", "Git failed."),
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
        "O modo de aprovação com que novas conversas começam. Um provedor sem ele usa o próximo modo mais seguro que oferece; uma conversa mantém o último modo usado.",
        "The approval mode new conversations start on. A provider without it uses the next safer mode it offers; a conversation keeps the mode it last used.",
    ),
    (
        "O motor do Sirus não respondeu.",
        "The Sirus engine did not respond.",
    ),
    (
        "O motor não gerou um código.",
        "The engine did not generate a code.",
    ),
    ("O pareamento foi recusado.", "Pairing was declined."),
    ("O pareamento não respondeu.", "Pairing did not respond."),
    ("O que um aparelho pode fazer", "What a device can do"),
    (
        "O sirus-host não foi encontrado.",
        "sirus-host was not found.",
    ),
    (
        "O terminal usa sempre o shell configurado no sistema.",
        "The PTY always launches $SHELL. This cannot be changed.",
    ),
    (
        "O uso do computador só está disponível no macOS.",
        "Computer use is available on macOS only.",
    ),
    ("Oceano", "Ocean"),
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
    ("Ontem", "Yesterday"),
    ("Ordem da lista", "List order"),
    ("Ordem das sessões", "Thread order"),
    ("Ordem dos projetos", "Project order"),
    (
        "Organiza as sessões dentro de cada projeto. As fixadas mantêm sua própria área.",
        "Orders sessions inside each project. Pinned sessions keep their own area.",
    ),
    ("Organização da sidebar", "Sidebar organization"),
    (
        "Os botões reagem ao ponteiro do mouse.",
        "Buttons react to the mouse pointer.",
    ),
    (
        "Os nomes e ícones dos provedores indicam onde cada cópia foi encontrada. Skills invocadas ficam disponíveis entre provedores. Os switches controlam o catálogo do Sirus Code; não alteram a descoberta nas CLIs externas.",
        "Provider names and icons show where each copy was found. Invoked skills are available across providers. The switches control the Sirus Code catalog; they do not change discovery in the external CLIs.",
    ),
    ("Outros modelos", "Other models"),
    ("Padrão", "Default"),
    ("Padrão do nome da branch", "Branch naming pattern"),
    ("Padrões", "Defaults"),
    (
        "Pagamentos, catálogo e pedidos",
        "Payments, catalog and orders",
    ),
    ("Painel Ambiente", "Environment panel"),
    ("Palavra por palavra", "Token by token"),
    ("Paleta de comandos", "Command Palette"),
    ("Parar", "Stop"),
    ("Parar agente", "Stop Agent"),
    ("Parar e enviar", "Stop and send"),
    ("Parar e revogar tudo", "Stop and revoke all"),
    (
        "Parar e transcrever no composer, ou parar e enviar a mensagem assim que for transcrita.",
        "Stop and transcribe into the composer, or stop and send the message once it is transcribed.",
    ),
    ("Pare quando quiser", "Stop anytime"),
    ("Parágrafos prontos", "Finished paragraphs"),
    ("Pasta de skills do Sirus Code", "Sirus Code skills folder"),
    ("Pasta do projeto", "Project folder"),
    (
        "Pasta onde os worktrees são criados. Escolha uma pasta ou digite o caminho.",
        "Folder where worktrees are created. Choose a folder or type the path.",
    ),
    (
        "Pastas no local dos worktrees que nenhuma sessão usa. Pastas com alterações não commitadas ficam; branches nunca são apagadas.{}",
        "Folders in the worktree location that no session uses. Folders with uncommitted changes stay; branches are never deleted.{}",
    ),
    (
        "Pede confirmação antes de fechar ou sair enquanto os agentes estão em execução.",
        "Ask before closing or quitting while agents are running.",
    ),
    ("Pedidos de permissão", "Permission requests"),
    ("Pedir aprovação", "Request approval"),
    ("Pergunta antes de agir.", "Asks before acting."),
    (
        "Pergunta antes de fechar uma aba do terminal; o shell e o histórico dela terminam.",
        "Ask before closing a terminal tab; its shell and history end.",
    ),
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
        "Personalize materiais, tipografia e movimento.",
        "Personalize materials, typography and motion.",
    ),
    (
        "Personalize os comandos de teclado que você usa no aplicativo.",
        "Customize the keyboard commands you use in the app.",
    ),
    ("Período", "Time"),
    ("Pessoal", "Personal"),
    ("Peça qualquer coisa", "Ask for anything"),
    ("Planejamento", "Planning"),
    ("Planejamento é só leitura", "Planning stays read-only"),
    ("Populares", "Popular"),
    ("Por projeto", "By project"),
    ("Precisa de aprovação", "Needs approval"),
    ("Preferências padrão", "Core defaults"),
    ("Preparado", "Staged"),
    ("Preparados", "Staged"),
    ("Preparar", "Stage"),
    ("Procurando…", "Searching…"),
    (
        "Procurar atualizações automaticamente",
        "Check for updates automatically",
    ),
    ("Produtividade", "Productivity"),
    ("Programação", "Coding"),
    ("Projeto", "Project"),
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
    ("Pronto", "Ready"),
    (
        "Proteção permanente. O Sirus Code não executa operações destrutivas do Git automaticamente.",
        "Cannot be turned off. Sirus Code never runs reset --hard, clean -fd, or worktree remove --force automatically.",
    ),
    ("Provedor", "Provider"),
    ("Provedor padrão", "Default provider"),
    ("Provedores", "Providers"),
    (
        "Pull requests, acompanhamento de PRs, correção de CI e pushes por HTTPS usam a conta ativa. As contas ficam guardadas no GitHub CLI (gh).",
        "Pull requests, PR watches, CI fixes and HTTPS pushes use the active account. Accounts are kept by GitHub CLI (gh).",
    ),
    ("Puxar da origem", "Pull from origin"),
    ("Quadros por segundo", "Frame rate"),
    ("Quadros, itens e atualizações", "Boards, items and updates"),
    (
        "Quando os checks do PR aberto de uma sessão falham, o agente dela recebe os erros e corrige; depois o Sirus Code faz commit e push do branch da sessão (nunca forçado). Até 3 tentativas por falha; senão pausa e avisa.",
        "When the checks of a session's open pull request fail, its agent gets the errors and fixes them, then Sirus Code commits and pushes the session branch (never forced). Up to 3 attempts per failure; it pauses and notifies you otherwise.",
    ),
    (
        "Quando outra pessoa abre um pull request ou pede sua revisão no repositório de um projeto salvo. Verificado ao abrir o Sirus Code e quando você volta para ele.",
        "When someone else opens a pull request or asks for your review in a saved project's repository. Checked when Sirus Code opens and when you come back to it.",
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
    (
        "Reabre a sessão criada mais recentemente no último projeto. As outras sessões continuam na barra lateral; os agentes não iniciam automaticamente.",
        "Reopen the newest-created session in the last project. Other sessions remain in the sidebar; agents do not start automatically.",
    ),
    (
        "Reabre o último projeto ativo ao iniciar o Sirus Code.",
        "Automatically reopen the last active project when Sirus Code starts.",
    ),
    ("Reabrir guia fechada", "Reopen closed tab"),
    ("Reabrir sessão mais nova", "Reopen newest session"),
    ("Reabrir último projeto", "Reopen last project"),
    ("Recentes", "Recent"),
    ("Recolher turnos terminados", "Fold finished turns"),
    ("Recusar", "Decline"),
    ("Reduzir movimento", "Reduce motion"),
    (
        "Remove sessões encerradas e seus worktrees isolados. Worktrees com alterações são preservados.",
        "Removes isolated worktrees for completed, failed, or stopped sessions. Dirty trees are refused without --force.",
    ),
    ("Remover", "Remove"),
    ("Remover da lista", "Remove from list"),
    ("Remover projeto?", "Remove project?"),
    ("Renomear", "Rename"),
    ("Renova em {}", "Renews in {}"),
    ("Repositório", "Repository"),
    (
        "Restaura a barra lateral e as larguras dos painéis.",
        "Clears sidebar collapse and panel widths in this session.",
    ),
    ("Restaurar", "Reset"),
    (
        "Restaurar configurações do Sirus Code",
        "Reset Sirus Code settings",
    ),
    ("Restaurar layout da janela", "Reset window layout"),
    ("Restaurar padrões", "Restore defaults"),
    ("Restaurar preferências", "Reset preferences"),
    (
        "Restaurar preferências não apaga pastas de projetos. Requer confirmação.",
        "Resetting settings does not delete projects on disk. Confirmation is required.",
    ),
    (
        "Retomar sozinho quando o limite de uso liberar",
        "Resume by itself when a usage limit resets",
    ),
    (
        "Revise o diff e faça commit.",
        "Review the diff and commit.",
    ),
    ("Revisão", "Review"),
    ("Revisão automática", "Auto-review"),
    ("Revogar", "Revoke"),
    ("Rode comandos no projeto.", "Run commands in the project."),
    ("Rode o app num aparelho.", "Run the app on a device."),
    ("Rápida", "Fast"),
    ("Sair", "Sign out"),
    (
        "Salva contagens limitadas do ciclo de execução localmente. Prompts, output, paths e credenciais são excluídos.",
        "Stores bounded lifecycle counts locally. Prompts, output, paths and credentials are excluded.",
    ),
    ("Saturação do fundo", "Background saturation"),
    (
        "Se o Sirus Code fechar inesperadamente enquanto um agente trabalha, a conversa continua na próxima abertura, pedindo que o agente confira primeiro o que já aconteceu. No máximo três vezes seguidas; conversas paradas, adiadas ou aguardando você ficam como estão.",
        "If Sirus Code closes unexpectedly while an agent works, that conversation continues on the next launch, asking the agent to check what already happened first. At most three times in a row; stopped, snoozed and waiting conversations are left alone.",
    ),
    (
        "Se outro app já usa a combinação, escolha outra.",
        "If another app already uses the combination, choose another.",
    ),
    ("Segurança", "Safety"),
    ("Sem", "None"),
    ("Sem conversas", "No conversations"),
    ("Sem descrição informada.", "No description provided."),
    ("Sem diferenças para mostrar.", "No differences to show."),
    ("Sem esforço", "None"),
    ("Sem login", "Not signed in"),
    ("Semanal · Opus", "Weekly · Opus"),
    ("Semanal · Sonnet", "Weekly · Sonnet"),
    ("Semanal · apps OAuth", "Weekly · OAuth apps"),
    ("Semanal · modelo reserva", "Weekly · reserve model"),
    ("Sempre bloqueados", "Always blocked"),
    (
        "Sempre ligado: excluir uma conversa apaga o histórico dela.",
        "Always on: deleting a conversation removes its history.",
    ),
    ("Servidores locais", "Local servers"),
    ("Sessão", "Session"),
    (
        "Sessões trabalhando, esperando você ou concluídas sem serem vistas ficam acima da ordem das sessões. As listas não se mexem enquanto o ponteiro está sobre elas.",
        "Sessions that are working, waiting for you or finished unseen go above the thread order. Lists hold still while your pointer is over them.",
    ),
    ("Shell padrão", "Default shell"),
    ("Sim", "Yes"),
    ("Simulador", "Simulator"),
    ("Sistema", "System"),
    ("Sites, lojas e agendamentos", "Sites, stores and bookings"),
    ("Sites, páginas e CMS", "Sites, pages and CMS"),
    ("Skills compartilhadas", "Shared skills"),
    ("Skills do projeto", "Project skills"),
    ("Skills dos agentes", "Agent skills"),
    ("Skills portáteis", "Portable skills"),
    ("Sobre", "About"),
    ("Solicitar aprovação", "Request approval"),
    ("Sons", "Sounds"),
    ("Sons de notificação", "Notification sounds"),
    ("Suave", "Smooth"),
    ("Suavização de fontes", "Font smoothing"),
    ("Sublinhado", "Underline"),
    (
        "Sublinhado de ortografia e correções automáticas do macOS enquanto você escreve uma mensagem, em todas as conversas.",
        "macOS spelling underlines and automatic corrections while you write a message, in every conversation.",
    ),
    (
        "Só aparelhos na sua rede Tailscale alcançam este Mac, e cada um precisa de um QR code desta página.",
        "Only devices on your Tailscale network can reach this Mac, and each one needs a QR code from this page.",
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
    (
        "Também respeita prefers-reduced-motion do sistema.",
        "Also follows the system's prefers-reduced-motion.",
    ),
    ("Tarefa concluída", "Task completed"),
    ("Tarefas, projetos e metas", "Tasks, projects and goals"),
    (
        "Teclas digitadas na página aparecem embaixo do vídeo. Campos de senha nunca aparecem.",
        "Keys typed on the page appear under the video. Password fields never appear.",
    ),
    ("Tema", "Theme"),
    ("Tema do workspace", "Workspace theme"),
    ("Tipografia da interface", "Interface typography"),
    ("Tipografia de código", "Code typography"),
    ("Tipografia do terminal", "Terminal typography"),
    ("Tirar da pasta", "Remove from folder"),
    ("Todas", "All"),
    ("Todas as conversas", "All conversations"),
    ("Todas as sessões", "All sessions"),
    ("Todo o período", "All time"),
    ("Todos os checkouts", "All checkouts"),
    ("Todos os projetos", "All projects"),
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
    ("Trocar", "Change"),
    ("Trocar de projeto", "Switch project"),
    ("Trocar modelo e esforço", "Change model and effort"),
    ("Trocar token", "Replace token"),
    ("Trocar…", "Switch…"),
    ("Título do commit", "Commit title"),
    (
        "Um aparelho conectado pode fazer o que esta janela faz: abrir sessões, enviar mensagens, aprovar ações, fazer commit e push. Seletores de arquivo, ditado e controle do computador ficam só no Mac. Remova o aparelho que não usar mais.",
        "A connected device can do what this window does: start sessions, send messages, approve actions, commit and push. Pickers, dictation and computer use stay on the Mac. Remove a device you no longer use.",
    ),
    (
        "Um círculo marca cada clique no vídeo.",
        "A ring marks each click in the video.",
    ),
    (
        "Um toque suave quando um agente termina um turno ou precisa de você. Não toca para a conversa que você está vendo.",
        "A soft chime when an agent finishes a turn or needs you. Not played for the conversation you are viewing.",
    ),
    (
        "Uma conversa do Claude com mais de 200 mil tokens parada compacta uns dois minutos antes do cache do prompt expirar (1 hora na assinatura, 5 minutos com chave de API), assim ao voltar a conversa inteira não é reenviada. Aparece como um turno /compact; conversas adiadas ficam como estão.",
        "A Claude conversation over 200,000 tokens that sits idle compacts about two minutes before its prompt cache expires (1 hour on a subscription, 5 minutes with an API key), so coming back does not resend the whole conversation. It shows as a /compact turn; snoozed conversations are left alone.",
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
    ("Usar a ferramenta {}", "Use the {} tool"),
    ("Usar esta conta", "Use this account"),
    ("Usar fonte do sistema na interface", "Use system UI font"),
    ("Usar shell padrão do sistema", "Use system default shell"),
    ("Usar uma skill", "Use a skill"),
    (
        "Use o Sirus Code pelo celular ou por outro computador. O trabalho continua rodando neste Mac; o outro aparelho só controla, pela sua rede privada da Tailscale.",
        "Use Sirus Code from your phone or another computer. The work still runs on this Mac; the other device only drives it, over your private Tailscale network.",
    ),
    ("Uso do {}", "{} usage"),
    ("Uso incluído", "Included usage"),
    ("Usou", "Used"),
    (
        "Vale para código nas transcrições, diffs e no editor.",
        "Applies to code in transcripts, diffs and the editor.",
    ),
    (
        "Variáveis: {session-name} {id}. O identificador mantém branches únicas.",
        "Variables: {session-name} {id}. The identifier keeps branches unique.",
    ),
    (
        "Velocidade do corredor do composer",
        "Composer runner speed",
    ),
    ("Ver uso do provedor", "View provider usage"),
    (
        "Verifica em segundo plano se Codex, Claude e outras CLIs têm versões mais novas.",
        "Check Codex, Claude and other provider CLIs for newer versions in the background.",
    ),
    ("Verificar atualizações das CLIs", "Check for CLI updates"),
    ("Verificar de novo", "Check again"),
    ("Versão", "Version"),
    ("Vidro fumê", "Smoked glass"),
    ("Visibilidade na sessão", "Session visibility"),
    ("Visibilidade no chat vazio", "Empty chat visibility"),
    ("Visualização", "View"),
    ("Voltar", "Back"),
    ("Voltar ao app", "Back to app"),
    (
        "Vários agentes trabalhando juntos",
        "Several agents working together",
    ),
    ("Workers, KV, R2 e D1", "Workers, KV, R2 and D1"),
    ("Workspace da sessão", "Session workspace"),
    ("Workspace padrão da sessão", "Default session workspace"),
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
    ("falhou", "failed"),
    ("feito", "done"),
    ("ferramenta", "tool"),
    ("ferramentas", "tools"),
    ("liberado", "allowed"),
    ("recusado", "declined"),
    ("trabalhando", "working"),
    ("{} agentes", "{} agents"),
    ("{} ainda não está pronto", "{} is not ready yet"),
    ("{} arquivos", "{} files"),
    ("{} de {}", "{} of {}"),
    (
        "{} de {} skills ativas · Em todas as origens",
        "{} of {} skills active · From all sources",
    ),
    ("{} ferramentas", "{} tools"),
    ("{} fps · pior quadro {} ms", "{} fps · worst frame {} ms"),
    ("{} mantidas", "{} kept"),
    ("{} restante", "{} left"),
    ("{} trabalhando", "{} working"),
    ("{} trabalhou por {}m {}s", "{} worked for {}m {}s"),
    ("{} trabalhou por {}s", "{} worked for {}s"),
    ("{} usado", "{} used"),
    ("{} · Encontrada em: {}", "{} · Found in: {}"),
    ("Ícone do Dock", "Dock icon"),
    ("Últimos 30 dias", "Last 30 days"),
    ("Últimos 7 dias", "Last 7 days"),
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
