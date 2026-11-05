//! Sample projects and conversations; nothing is read from or written to disk.
use std::time::Instant;

pub struct Step {
    pub label: &'static str,
    pub detail: &'static str,
    pub seconds: f32,
}

pub struct Turn {
    pub user: String,
    pub prose: &'static str,
    pub steps: &'static [Step],
    pub answer: String,
    /// Set while this turn plays out; none once it was history from the start.
    pub started: Option<Instant>,
    pub open: bool,
    pub toggled: Option<Instant>,
    pub steps_open: bool,
    pub steps_toggled: Option<Instant>,
}

pub struct Conversation {
    pub title: String,
    pub provider: &'static str,
    pub age: &'static str,
    pub turns: Vec<Turn>,
}

pub struct Project {
    pub name: &'static str,
    pub open: bool,
    pub pinned: bool,
    pub conversations: Vec<usize>,
}

pub const THINKING: f32 = 1.4;
pub const WORDS_PER_SECOND: f32 = 45.;

pub const STEPS: &[&[Step]] = &[
    &[
        Step { label: "Lendo", detail: "src/components/SessionPane.tsx", seconds: 0.9 },
        Step { label: "Buscando", detail: "text-wrap em src/styles", seconds: 0.7 },
        Step { label: "Lendo", detail: "src/styles/composer.css", seconds: 0.8 },
        Step { label: "Editando", detail: "src/styles/composer.css", seconds: 1.3 },
        Step { label: "Executando", detail: "npm run typecheck", seconds: 2.1 },
    ],
    &[
        Step { label: "Lendo", detail: "src-tauri/src/github_accounts.rs", seconds: 0.8 },
        Step { label: "Executando", detail: "gh auth status --json hosts", seconds: 1.2 },
        Step { label: "Editando", detail: "src/components/settings/SettingsPanels.tsx", seconds: 1.1 },
    ],
    &[
        Step { label: "Buscando", detail: "useBrowserBounds", seconds: 0.6 },
        Step { label: "Lendo", detail: "src/lib/use-browser-bounds.ts", seconds: 0.9 },
        Step { label: "Editando", detail: "src/components/BrowserDevice.tsx", seconds: 1.4 },
        Step { label: "Executando", detail: "cargo check", seconds: 2.4 },
    ],
];

const PROSE: &[&str] = &[
    "Vou começar pelo campo do composer e pelo estilo de texto que ele herda.",
    "Vou ver como o GitHub CLI guarda as contas e o que dá para mostrar na tela.",
    "Vou olhar como o navegador é escondido quando um menu cobre a área dele.",
    "Vou ler o diff inteiro e rodar as verificações.",
];
const QUESTIONS: &[&str] = &[
    "O composer fica estranho quando tem muito texto, as palavras pulam de linha enquanto digito. Pode ver?",
    "Dá para trocar de conta do GitHub dentro do app, sem ir ao terminal?",
    "O navegador fica em branco quando abro um menu por cima dele. O que está acontecendo?",
    "Faz uma revisão desse diff antes de eu mandar para o main.",
];
const ANSWERS: &[&str] = &[
    "Achei a causa. O campo herda text-wrap: pretty do estilo das mensagens, e o WebKit recalcula as quebras das linhas de cima a cada tecla. Troquei por uma quebra simples, e o texto já digitado fica parado. As mensagens do chat continuam como estavam.",
    "Dá sim. Listei as contas com gh auth status e cada uma agora tem um botão para ativar ou sair. Adicionar conta mostra o código de uso único direto na tela, e a lista atualiza sozinha quando o login termina.",
    "O WKWebView desenha por cima de todo o DOM, então escondemos ele quando um menu cobre a área. Agora guardo uma imagem da página antes de esconder e mostro no lugar, assim o fundo não some.",
    "Revisei os arquivos alterados. Não achei erro de lógica, só um estado que pode ser simplificado. Os testes passam e o typecheck está limpo, pode mandar.",
];

pub fn turn(index: usize, started: Option<Instant>) -> Turn {
    Turn {
        user: QUESTIONS[index % QUESTIONS.len()].into(),
        prose: PROSE[index % PROSE.len()],
        steps: STEPS[index % STEPS.len()],
        answer: ANSWERS[index % ANSWERS.len()].into(),
        started,
        open: started.is_some(),
        toggled: None,
        steps_open: false,
        steps_toggled: None,
    }
}

pub fn sample() -> (Vec<Project>, Vec<Conversation>) {
    let conversations: Vec<(&str, &str, &str, usize)> = vec![
        ("Composer pula palavras ao digitar", "claude", "agora", 80),
        ("Contas do GitHub nas configurações", "claude", "2 h", 12),
        ("Navegador em branco atrás do menu", "opencode", "5 h", 24),
        ("Hierarquia da sidebar", "claude", "ontem", 8),
        ("Tela de pedidos não atualiza", "codex", "1 d", 16),
        ("Migração das tabelas de produtos", "claude", "2 d", 6),
        ("Documentos da viagem", "claude", "3 d", 10),
        ("Build do app Android", "opencode", "4 d", 4),
        ("Página de preços", "claude", "6 d", 6),
    ];
    let conversations: Vec<Conversation> = conversations
        .into_iter()
        .map(|(title, provider, age, turns)| Conversation {
            title: title.into(),
            provider,
            age,
            turns: (0..turns).map(|i| turn(i, None)).collect(),
        })
        .collect();
    let projects = vec![
        Project { name: "sirus-code", open: true, pinned: false, conversations: vec![0, 1, 2, 3] },
        Project { name: "loja-online", open: true, pinned: false, conversations: vec![4, 5] },
        Project { name: "documentos", open: true, pinned: false, conversations: vec![6] },
        Project { name: "sirus-mobile", open: false, pinned: false, conversations: vec![7] },
        Project { name: "site", open: false, pinned: false, conversations: vec![8] },
    ];
    (projects, conversations)
}

/// Sample files per project, for the top bar's search. (name, folder)
pub fn files(project: &str) -> &'static [(&'static str, &'static str)] {
    match project {
        "sirus-code" => &[
            ("AGENTS.md", ""), ("package.json", ""), ("README.md", ""), (".gitignore", ""),
            ("AgentComposer.tsx", "src/components"), ("ComposerEditor.tsx", "src/components"), ("RightDock.tsx", "src/components"),
            ("EnvironmentPanel.tsx", "src/components"), ("ConversationSidebar.tsx", "src/components"), ("composer.css", "src/styles"),
            ("index.css", "src/styles"), ("use-browser-bounds.ts", "src/lib"), ("turn-window.ts", "src/lib"),
            ("lib.rs", "src-tauri/src"), ("github_accounts.rs", "src-tauri/src"), ("skills.rs", "src-tauri/src"), ("connectors.rs", "src-tauri/src"),
        ],
        "loja-online" => &[
            ("AGENTS.md", ""), ("package.json", ""), (".env.example", "apps/web"), ("README.md", "apps/web"),
            ("components.json", "apps/web"), ("page.tsx", "apps/web/src/app"), ("layout.tsx", "apps/web/src/app"), ("pedidos.ts", "apps/web/src/lib"),
        ],
        _ => &[("README.md", ""), ("package.json", ""), ("index.html", "src"), ("main.ts", "src"), ("styles.css", "src")],
    }
}
