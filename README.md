# Screen Share POC — testar dentro de uma Discord Activity real

Este guia contém **apenas** o necessário para você validar o POC de transporte
(vídeo H.264 → relay → Activity → WebCodecs) dentro do Discord. Não há captura de
tela, áudio nem encoder — o cliente de teste envia um clipe H.264 pré-gravado.

## Peças

| Componente | Onde roda | Papel |
| --- | --- | --- |
| `activity/` | GitHub Pages (HTTPS) | viewer: WebSocket + WebCodecs |
| `relay` | seu servidor (WSS) | roteia bytes streamer → viewer |
| `test-client` | seu PC Windows | publica o clipe `assets/test.h264` |

Fluxo:

```text
test-client (Windows) --ws--> relay --wss--> Discord proxy --wss--> Activity (WebCodecs)
```

---

## 1. Hospedar a Activity no GitHub Pages

1. Faça push do repositório para o GitHub.
2. Em **Settings → Pages**, selecione **GitHub Actions** como source.
   O workflow [.github/workflows/deploy-activity.yml](.github/workflows/deploy-activity.yml)
   publica a pasta `activity/` automaticamente a cada push na `main`.
3. Anote a URL publicada, algo como `https://SEU_USUARIO.github.io/SEU_REPO/`.
   - O arquivo `activity/.nojekyll` já está incluído para o Pages servir os
     arquivos sem processamento Jekyll.

> Observação: a Activity é servida da raiz do artefato (`activity/`), então
> `index.html`, `main.js` e `config.js` ficam em `/`.

## 2. Colocar o relay acessível via WSS

O relay precisa ser alcançável por `wss://` (TLS). Duas opções:

**Opção A — TLS terminado no próprio relay:**

```powershell
$env:RELAY_ADDR   = "0.0.0.0:9443"
$env:RELAY_TLS_CERT = "/caminho/fullchain.pem"
$env:RELAY_TLS_KEY  = "/caminho/privkey.pem"
./relay
```

Sem as variáveis `RELAY_TLS_*`, o relay serve `ws://` puro (útil só em local).

**Opção B — TLS terminado por um proxy reverso** (nginx/caddy/cloudflared) na
frente do relay em `ws://`. O relay não muda; só o alvo do URL mapping precisa
ser `wss`.

> Para um teste rápido sem servidor próprio, um túnel funciona:
> `cloudflared tunnel --url http://localhost:9000` gera uma URL pública HTTPS/WSS
> que encaminha para o relay local. Use o host retornado como TARGET no passo 3.

## 3. Configurar o Discord Developer Portal

1. Crie uma aplicação em https://discord.com/developers/applications.
2. **Activities → Settings**: habilite Activities. Em **Supported Platforms**,
   marque as plataformas onde vai testar (Web e/ou Desktop).
3. **Activities → URL Mappings** (https://discord.com/developers/applications/select/embedded/url-mappings):

   | PREFIX | TARGET | Observação |
   | --- | --- | --- |
   | `/relay` | `seu-relay.exemplo.com` | host do relay WSS — **sem** `wss://` na frente |
   | `/` | `SEU_USUARIO.github.io/SEU_REPO` | a Activity no GitHub Pages |

   Regras importantes (da doc oficial):
   - O TARGET **não** inclui protocolo (nada de `https://` ou `wss://`).
   - O TARGET aponta para um **diretório**, não um arquivo.
   - Se dois prefixos compartilham o início do caminho, coloque o **mais longo
     primeiro**. Aqui `/relay` deve vir **antes** de `/` na lista.

4. Copie o **Application ID** (client id).

## 4. Apontar a Activity para seu client id

Edite [activity/config.js](activity/config.js):

```js
window.POC_CONFIG = {
  DISCORD_CLIENT_ID: "SEU_APPLICATION_ID",
  RELAY_PROXY_PREFIX: "/relay", // deve casar com o PREFIX do passo 3
  ROOM: "test-room",
  STREAM: "test-stream",
};
```

Faça commit/push (o Pages redeploya). Dentro do Discord, a Activity conecta em:

```text
wss://{clientId}.discordsays.com/.proxy/relay/watch/test-room/test-stream
```

O `/.proxy` é adicionado pelo código e removido pelo proxy do Discord; o restante
(`/relay/...`) casa com o seu URL mapping e chega ao relay como
`/watch/test-room/test-stream`.

## 5. Publicar o vídeo de teste (Windows)

Com o relay no ar, rode o cliente de teste apontando para o **mesmo host WSS**
que você mapeou (ou o `ws://` local se estiver testando fora do Discord):

```powershell
# através do host público do relay (mesmo destino do URL mapping)
./test-client wss://seu-relay.exemplo.com/publish/test-room/test-stream assets/test.h264 --loop
```

`--loop` mantém o clipe repetindo, útil para observar late-join.

> O `test-client` usa o mesmo módulo `stream-core` (transport + protocolo) que o
> app real usará; ele apenas substitui captura/encode por leitura de arquivo.

## 6. Abrir a Activity no Discord

1. Ative **Developer Mode** (User Settings → Advanced).
2. Entre em um canal de voz.
3. Abra a bandeja de Activities (botão de foguete) e selecione sua aplicação.
4. No painel lateral da Activity, observe os diagnósticos:
   - **State**: `CONNECTED`
   - **Packets / Bytes / Frames decoded** subindo
   - **Resolution**: `640x360`
   - O canvas deve mostrar o padrão de barras coloridas (testsrc2).

Logs da Activity: no desktop, DevTools do client (PTB: `View → Developer →
Toggle Developer Tools`); no mobile, User Settings → Debug Logs (filtre pelo
Application ID).

---

## Critério de sucesso

O POC está confirmado quando, **dentro do Discord real**, "Frames decoded" sobe e
o vídeo aparece no canvas. Se não subir, verifique nesta ordem:

1. O URL mapping `/relay` está **antes** de `/` e o TARGET é o host WSS correto
   (sem protocolo).
2. O relay está acessível por `wss://` de fora (teste com um cliente WS externo).
3. DevTools da Activity: erro `blocked:csp` indica mapping ausente/errado;
   erro de WebSocket indica relay inacessível.
4. Tamanho de frame: o clipe de teste tem keyframes ~12 KB. Se depois você usar
   resoluções altas, keyframes grandes podem revelar limites do proxy — este é
   justamente um dos pontos a medir.

## Rodar tudo localmente (sem Discord)

```powershell
# 1. relay (ws puro)
cargo run -p relay

# 2. publisher
cargo run -p desktop --bin test-client -- ws://127.0.0.1:9000/publish/test-room/test-stream assets/test.h264 --loop

# 3. viewer no navegador
node tools/static-server.js
# abra http://127.0.0.1:8080/index.html?relay=ws://127.0.0.1:9000
```

O parâmetro `?relay=` sobrepõe o destino, permitindo testar o viewer fora do
Discord contra um relay local.
