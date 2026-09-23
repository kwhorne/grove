"""The course text. One dict per chapter; build.py wraps them.

Rules for anyone editing this file:
  - Every claim must be something Grove actually does. When in doubt, check
    docs/INSTALL.md, docs/COMMANDS.md and docs/ARCHITECTURE.md rather than
    guessing.
  - Say why before what. A step nobody understands is a step nobody repeats.
  - Name the limits where they bite, not in a disclaimer at the end.
"""

COURSE_TITLE = "The Perfect Setup"

COURSE_LEAD = """
    Local development with Grove, from a machine with nothing on it to a
    team that shares one environment. Fourteen chapters that build the setup
    you would have designed yourself if you had ever had a free week &mdash;
    and explain why each piece is there before showing which command puts it
    in place.
"""

COURSE_INTRO = """
                <h2 id="who-this-is-for">Who this is for</h2>
                <p>
                    Anyone who runs web projects on a Mac and has felt the
                    weight of their own machine. You will get the most from it
                    if you work with PHP &mdash; Laravel, WordPress, plain PHP
                    &mdash; but the DNS, the certificates, the service
                    supervision and the request timeline are the same whatever
                    is behind them.
                </p>
                <p>
                    The running example is <strong>Freddy</strong>, a small
                    notes app. Substitute your own project everywhere you see
                    his; nothing here depends on how he was built.
                </p>

                <h2 id="what-you-need">What you need</h2>
                <p>
                    A Mac on Apple silicon or Intel, and administrator access
                    for two commands in chapter 2. Linux support is in beta and
                    the commands are the same. Nothing else is required: Grove
                    brings its own PHP, Node, databases and mail server, so
                    Homebrew is optional and Herd and Valet are replaceable
                    &mdash; chapter 7 removes them.
                </p>

                <h2 id="how-to-read-it">How to read it</h2>
                <p>
                    Every chapter is built the same way: <em>the problem</em>
                    (why this exists at all), <em>the hard way</em> (what you
                    would do without it, and why that goes wrong), then the
                    work. The order matters &mdash; chapter 13 makes your
                    environment reproducible, and it can only do that because
                    chapters 3 to 8 decided what there is to reproduce.
                </p>
"""

CHAPTERS = [
    # ------------------------------------------------------------------ 1
    {
        "n": 1,
        "title": "What a Local Environment Actually Is",
        "summary": "The five jobs every dev machine does, why they are usually five unrelated tools, and what changes when one process owns them.",
        "lead": """
            Before installing anything, it is worth naming the parts. A local
            development environment is not one thing &mdash; it is five jobs
            that happen to run on the same laptop, and almost every problem
            you have had with yours came from the seams between them.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    A working machine is invisible. You type
                    <code>myapp.test</code>, a page appears, and no part of
                    your attention goes to how. That is exactly as it should
                    be &mdash; and it is why, when something breaks, nobody
                    has any idea where to look. The environment was never
                    designed; it accumulated.
                </p>
                <p>
                    So here are the five jobs, because knowing them is what
                    turns &ldquo;it doesn&#039;t work&rdquo; into a question
                    with an answer.
                </p>
                <ol>
                    <li>
                        <strong>A name resolver.</strong> Something must
                        decide that <code>myapp.test</code> means your
                        machine. That is DNS, and it is the first thing that
                        breaks.
                    </li>
                    <li>
                        <strong>A web server.</strong> Something must accept
                        the request on port 80 or 443, work out which project
                        it belongs to, and hand it to the right runtime.
                    </li>
                    <li>
                        <strong>Language runtimes.</strong> PHP, Node &mdash;
                        and not one of each, but the specific versions each
                        project needs, at the same time.
                    </li>
                    <li>
                        <strong>Backing services.</strong> A database, a
                        cache, a queue, somewhere for outgoing mail to land
                        instead of a real inbox.
                    </li>
                    <li>
                        <strong>Certificates.</strong> Because half of what
                        you build now refuses to work over plain HTTP.
                    </li>
                </ol>

                <h2 id="the-hard-way">The hard way</h2>
                <p>
                    The traditional answer is one tool per job, assembled over
                    years by different people with different opinions:
                    Homebrew for PHP, nvm for Node, Docker for the database,
                    a line in <code>/etc/hosts</code> for the name, nginx or
                    Apache configured by hand, mkcert for a certificate, and
                    MailHog because somebody once sent a test email to a real
                    customer.
                </p>
                <p>
                    Each piece is fine. The seams are where you lose the days:
                </p>
                <ul>
                    <li>
                        <strong>Nothing knows about anything else.</strong>
                        You upgrade PHP with Homebrew and your nginx config
                        still points at the old socket. Nothing warns you;
                        the site just stops.
                    </li>
                    <li>
                        <strong>Versions are global.</strong> One PHP at a
                        time, so the client project on 8.1 and the new one on
                        8.5 cannot both run &mdash; and switching is a
                        ceremony you perform several times a day.
                    </li>
                    <li>
                        <strong>Nothing is reproducible.</strong> A new
                        colleague spends two days recreating your machine
                        from a README nobody has updated since the last time
                        someone new joined.
                    </li>
                    <li>
                        <strong>Everything is a little bit root.</strong>
                        A dozen installers have each asked for your password
                        for their own reasons, and no one process is
                        accountable for the result.
                    </li>
                </ul>
                <p>
                    The <code>/etc/hosts</code> line deserves a paragraph of
                    its own, because it is the most common approach and the
                    worst. It maps exactly one name. Every new project is
                    another manual edit with <code>sudo</code>, wildcards are
                    impossible, and the file is invisible from inside any
                    project &mdash; so the reason a site stopped resolving is
                    in a system file nobody thinks to open.
                </p>

                <h2 id="one-supervisor">What changes with one supervisor</h2>
                <p>
                    Grove does all five jobs in one supervised process, and
                    the difference is not convenience. It is that the parts
                    can <em>know about each other</em>.
                </p>
                <ul>
                    <li>
                        It answers DNS for your TLD, so a new project needs
                        no file edited anywhere &mdash; the name exists
                        because the folder does.
                    </li>
                    <li>
                        It is the web server, so it knows which PHP version a
                        site is pinned to and routes to that runtime without
                        a config file to keep in sync.
                    </li>
                    <li>
                        It installs the runtimes and the databases, so
                        upgrading one does not break another&#039;s idea of
                        where it lives.
                    </li>
                    <li>
                        It owns the certificate authority, so a new HTTPS
                        site is one command rather than a ceremony.
                    </li>
                    <li>
                        It sees every request, which is why chapters 10 and
                        12 are possible at all &mdash; nothing assembled from
                        separate tools can show you the timeline of a request
                        with the queries it caused.
                    </li>
                </ul>

                <h2 id="the-arc">What we are building</h2>
                <p>
                    By chapter 14 you will have this, and every part of it
                    will be something you chose rather than something that
                    accumulated:
                </p>
                <ul>
                    <li>
                        Every project in one folder served at
                        <code>https://&lt;name&gt;.test</code> with a trusted
                        padlock, no config per project.
                    </li>
                    <li>
                        Several PHP versions at once, pinned per site, with
                        the extensions you need &mdash; including ones no
                        prebuilt binary has.
                    </li>
                    <li>
                        Databases, cache and a mail-catcher that Grove
                        installs and supervises, with snapshots you can roll
                        back.
                    </li>
                    <li>
                        A terminal whose <code>php</code> and
                        <code>composer</code> follow the project you are
                        standing in.
                    </li>
                    <li>
                        A <code>grove.toml</code> in the repository, so a new
                        colleague goes from <code>git clone</code> to a
                        running identical environment with one command.
                    </li>
                </ul>
                <div class="callout">
                    <strong>One honest note before we start.</strong> Grove
                    replaces a stack you may have spent years arranging, and
                    that is worth doing deliberately rather than all at once.
                    Chapter 7 is where Herd and Valet come off, and it comes
                    <em>after</em> the chapters that make sure everything you
                    relied on is in place. Nothing here asks you to burn the
                    boats early.
                </div>
        """,
        "learned": [
            "<strong>A local environment is five jobs:</strong> a name resolver, a web server, runtimes, services and certificates.",
            "<strong>The seams are where the days go</strong> &mdash; one tool per job means nothing knows about anything else.",
            "<strong>/etc/hosts maps one name</strong> and hides the reason a site stopped working in a system file.",
            "<strong>One supervisor lets the parts know about each other,</strong> which is what makes per-site PHP and a request timeline possible at all.",
            "<strong>Replace deliberately.</strong> Herd and Valet come off in chapter 7, after everything you relied on is in place.",
        ],
        "next": "you install Grove &mdash; and find out precisely what the two commands that ask for your password are doing with it.",
    },
    # ------------------------------------------------------------------ 2
    {
        "n": 2,
        "title": "Install, and What Needs Root",
        "summary": "Two privileged commands, what each one writes, and why the daemon that serves your sites is not root at all.",
        "lead": """
            Installing takes two commands and about a minute. This chapter
            spends longer than that explaining what they do, because a tool
            that asks for your administrator password has an obligation to be
            legible &mdash; and because Grove&#039;s answer here is unusual.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Developer tools ask for <code>sudo</code> casually. An
                    installer wants it, a package manager wants it, a helper
                    daemon wants it forever. Each time the justification is
                    something small and true &mdash; a port, a system file
                    &mdash; and the real cost never appears on the invoice:
                    every bug in that process is now a root bug on your
                    machine.
                </p>
                <p>
                    You should know what you are agreeing to. So here it is,
                    line by line.
                </p>

                <h2 id="two-commands">The two commands</h2>
                <pre><code>sudo grove init       # config, root CA, a PHP build, resolver + trust
sudo grove install    # the background service</code></pre>
                <p>
                    <strong><code>grove init</code></strong> does four things.
                    It writes <code>config.toml</code>. It creates a local
                    certificate authority and adds it to your system trust
                    store &mdash; that is chapter 4, and it is the part that
                    needs your password. It downloads a PHP build. And it
                    writes <code>/etc/resolver/test</code>, a small file that
                    tells macOS to send every <code>.test</code> lookup to
                    Grove instead of to the internet.
                </p>
                <p>
                    <strong><code>grove install</code></strong> writes the
                    launchd unit that keeps the daemon running and restarts it
                    if it dies. On Linux it writes a systemd unit instead.
                </p>

                <h2 id="not-root">The daemon that serves your sites is not root</h2>
                <p>
                    Here is the part worth reading twice, because it is the
                    opposite of what the ports suggest.
                </p>
                <p>
                    Grove serves on 80, 443 and 53, and binding a port below
                    1024 requires privilege. The obvious conclusion &mdash;
                    and the one Grove itself drew for its first several
                    versions &mdash; is that the daemon must be root. It does
                    not, because of one distinction:
                    <strong>binding a privileged port needs root; serving on one does not.</strong>
                </p>
                <p>
                    A socket is a file descriptor. Whoever binds it needs
                    privilege; whoever holds it afterwards needs none. And
                    your machine already has a root process whose entire job
                    is starting other processes. So launchd binds 53, 80 and
                    443 while it is root, then starts Grove <em>as you</em>
                    and hands the already-listening descriptors over.
                </p>
                <pre><code>grove doctor</code></pre>
                <pre><code>✓ privileges     http_port=80, elevated=false, sockets from the
                 service manager: tcp/53, tcp/80, tcp/443, udp/53</code></pre>
                <p>
                    <code>elevated=false</code> is your name on that line.
                    From its first instruction, the process handling every
                    request to your sites is running as you &mdash; and so is
                    everything it starts: PHP-FPM, PostgreSQL, MySQL, Redis,
                    the Vite server, Composer.
                </p>
                <p>
                    What still needs root is one-off and visible: writing the
                    unit file, writing <code>/etc/resolver</code>, and adding
                    the CA to the trust store. All three happen inside the two
                    commands above, while you are watching, having typed
                    <code>sudo</code> on purpose. Nothing that happens
                    <em>while a request is being served</em> is privileged.
                </p>

                <h2 id="doctor">Learn to run doctor</h2>
                <p>
                    <code>grove doctor</code> is the command to reach for
                    before you start guessing, and it is worth running once
                    now while everything is fresh, so you know what healthy
                    looks like.
                </p>
                <p>
                    It checks the things that are true or false rather than
                    matters of opinion: the daemon is running, the ports are
                    listening, the resolver file exists and points at Grove,
                    the CA is trusted by the system, the Grove home is owned
                    by you and not world-writable, your certificates have not
                    expired. Each line is a claim you could verify by hand;
                    the value is that it verifies all of them in a second.
                </p>
                <div class="callout">
                    <strong>The one upgrade note.</strong> Grove updates its
                    binary without rewriting the launchd unit. From 1.8.0 to
                    1.9.0 that meant an old machine kept running the daemon as
                    root; since 1.9.0 the daemon <em>refuses</em> to start as
                    root, so a machine that never ran the step below stops at
                    startup &mdash; and names the command that fixes it. That
                    command is <code>sudo grove install</code>, once. It
                    rewrites the unit so launchd binds the ports and starts
                    Grove as you, and it hands the Grove home to you in the
                    same step, which is why it is one command and not a
                    surprise permission error on Monday.
                </div>
        """,
        "learned": [
            "<strong>Two privileged commands, and both are legible.</strong> init writes config, CA, resolver; install writes the service unit.",
            "<strong>Binding a privileged port needs root; serving on one does not.</strong> launchd binds and hands the descriptor over.",
            "<strong>The daemon runs as you</strong> &mdash; and so does everything it starts. <code>elevated=false</code> in doctor is the proof.",
            "<strong>What still needs root is one-off and visible,</strong> and happens while you are watching.",
            "<strong>Run doctor once while healthy,</strong> so you know what healthy looks like.",
        ],
        "next": "your first site &mdash; and why a folder becoming a URL with no configuration is a different thing from a shortcut.",
    },
    # ------------------------------------------------------------------ 3
    {
        "n": 3,
        "title": "A Folder Becomes a Site",
        "summary": "Park, link, and the reason a real DNS resolver beats an /etc/hosts line every single time.",
        "lead": """
            One command and every project in a folder has a URL. This chapter
            is about what just happened &mdash; because &ldquo;it works with no
            configuration&rdquo; is a claim worth taking apart before you rely
            on it.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Between a folder of code and a working local URL there are
                    three questions, and the traditional answer to each is a
                    file you must remember to edit.
                </p>
                <ol>
                    <li>
                        <strong>What is this project called?</strong> A line
                        in <code>/etc/hosts</code>.
                    </li>
                    <li>
                        <strong>Where does the server send its
                        requests?</strong> A virtual host block.
                    </li>
                    <li>
                        <strong>What kind of project is it?</strong> Whether
                        the document root is the folder or
                        <code>public/</code>, whether URLs rewrite to
                        <code>index.php</code> &mdash; more config.
                    </li>
                </ol>
                <p>
                    Three answers per project, none of them interesting, all
                    of them yours to maintain forever.
                </p>

                <h2 id="park">Park: a folder of projects</h2>
                <pre><code>grove park ~/Code</code></pre>
                <p>
                    Every sub-folder is now a site.
                    <code>~/Code/freddy</code> is
                    <code>http://freddy.test</code>. Create
                    <code>~/Code/invoices</code> tomorrow and
                    <code>http://invoices.test</code> works immediately, with
                    nothing run and nothing edited.
                </p>
                <p>
                    That last sentence is the one to notice. The site exists
                    because the directory exists &mdash; there is no registry
                    to fall out of sync, no step to forget at eleven at night.
                    Park the folder you keep your work in, once, and the
                    question &ldquo;how do I set up a local URL&rdquo; stops
                    being a question you have.
                </p>

                <h2 id="link">Link: one project, from inside it</h2>
                <pre><code>cd ~/work/client-site
grove link                 # → client-site.test
grove link acme            # → acme.test</code></pre>
                <p>
                    For projects that live outside your parked folder.
                    <code>grove list</code> shows everything Grove serves,
                    linked and parked together.
                </p>

                <h2 id="dns">Why DNS, and not /etc/hosts</h2>
                <p>
                    Grove answers DNS for your whole TLD. macOS is told once,
                    in <code>/etc/resolver/test</code>, to ask Grove about
                    anything ending in <code>.test</code>, and Grove answers
                    <code>127.0.0.1</code> for all of it.
                </p>
                <p>
                    Compared with a hosts file, that buys four things:
                </p>
                <ul>
                    <li>
                        <strong>New names cost nothing.</strong> No file, no
                        <code>sudo</code>, no restart.
                    </li>
                    <li>
                        <strong>Subdomains work.</strong>
                        <code>api.freddy.test</code>,
                        <code>tenant-one.freddy.test</code> &mdash; multi-tenant
                        apps and subdomain routing work locally without a
                        hosts entry per tenant, which is impossible with the
                        other approach.
                    </li>
                    <li>
                        <strong>It is visible.</strong>
                        <code>grove list</code> and <code>grove doctor</code>
                        can tell you what is being served and whether the
                        resolver is in place. A hosts file is a system file
                        that nothing in your project can see.
                    </li>
                    <li>
                        <strong>It is reversible.</strong> Stop Grove and your
                        machine stops resolving <code>.test</code> &mdash;
                        there is no residue to clean up in six months.
                    </li>
                </ul>
                <p>
                    Use <code>.test</code> and nothing else, by the way. It is
                    reserved by the IETF for exactly this and will never be
                    bought by anyone. <code>.dev</code> was a popular choice
                    until Google bought it and turned on enforced HTTPS,
                    which broke a great many machines in an afternoon.
                </p>

                <h2 id="drivers">How it knows what your project is</h2>
                <p>
                    The third question &mdash; what kind of project is this
                    &mdash; Grove answers by looking. It detects Laravel,
                    WordPress, plain PHP, static sites and proxies, and serves
                    each the way that kind of project expects: the right
                    document root, the right rewrite rules.
                </p>
                <p>
                    Detection is a default, not a decree. When you are running
                    something Grove has never seen &mdash; a Node app on a
                    port, a container &mdash; say so:
                </p>
                <pre><code>grove proxy dashboard http://127.0.0.1:3000</code></pre>
                <p>
                    Now <code>dashboard.test</code> is your Node app, with the
                    same DNS and, from the next chapter, the same trusted
                    certificate as everything else. This is also how a Docker
                    or OrbStack container becomes a proper local site.
                </p>
                <div class="callout">
                    <strong>Try it now.</strong> Park your projects folder and
                    run <code>grove list</code>. Everything you have been
                    meaning to set up a local URL for has one, including the
                    projects you had forgotten were there.
                </div>
        """,
        "learned": [
            "<strong>Park a folder and every sub-folder is a site</strong> &mdash; the URL exists because the directory does.",
            "<strong>Link is for projects that live elsewhere,</strong> and both show up in <code>grove list</code>.",
            "<strong>A real resolver gives you wildcards, subdomains and reversibility</strong> that a hosts file cannot.",
            "<strong>Use .test.</strong> It is reserved for this; .dev was bought and broke everyone using it.",
            "<strong>Project type is detected, not configured</strong> &mdash; and <code>grove proxy</code> is the override for anything it has not seen.",
        ],
        "next": "HTTPS &mdash; why local development needs it now, and how a certificate authority on your own machine can be safe.",
    },
    # ------------------------------------------------------------------ 4
    {
        "n": 4,
        "title": "A Padlock That Means Something",
        "summary": "Why localhost needs HTTPS now, one command that provides it, and the constraint that makes a local CA safe.",
        "lead": """
            Half of what you build refuses to work over plain HTTP, and the
            usual fix &mdash; a self-signed certificate and a browser warning
            you click through &mdash; teaches you to ignore the warning that
            exists to protect you. There is a better answer, and it is one
            command.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Browsers have spent a decade moving features behind
                    &ldquo;secure context&rdquo;. Over plain HTTP you lose:
                </p>
                <ul>
                    <li>
                        <strong>Service workers.</strong> No offline, no push,
                        no PWA &mdash; they simply do not register.
                    </li>
                    <li>
                        <strong>Secure cookies.</strong>
                        <code>Secure</code> and <code>SameSite=None</code>
                        cookies are dropped, which means sessions that behave
                        one way locally and another in production.
                    </li>
                    <li>
                        <strong>Most OAuth providers.</strong> Many refuse to
                        register a non-HTTPS callback at all, so
                        &ldquo;log in with&hellip;&rdquo; cannot be tested
                        locally.
                    </li>
                    <li>
                        <strong>Clipboard, geolocation, camera, WebAuthn.</strong>
                        All secure-context only.
                    </li>
                </ul>
                <p>
                    And beyond the features: if you develop over HTTP and
                    deploy over HTTPS, mixed-content bugs and protocol-relative
                    URL mistakes are invisible until production. You are
                    testing a different thing from the one you ship.
                </p>

                <h2 id="the-hard-way">The hard way</h2>
                <p>
                    The usual workaround is a self-signed certificate, which
                    produces a full-page browser warning you click through
                    several times a day. That is worse than it looks, for two
                    reasons.
                </p>
                <p>
                    It <strong>trains the reflex</strong>. The interstitial
                    exists to stop you proceeding to a site that is lying
                    about who it is, and you are practising proceeding.
                </p>
                <p>
                    And it <strong>does not actually work</strong> for the
                    things you wanted. A service worker will not register
                    behind an untrusted certificate no matter how many times
                    you dismiss the warning. Neither will an API client that
                    validates certificates &mdash; so your app&#039;s own
                    server-to-server calls fail in ways that have nothing to
                    do with your code.
                </p>

                <h2 id="secure">One command</h2>
                <pre><code>grove secure freddy</code></pre>
                <pre><code>✓ freddy is now served over HTTPS → https://freddy.test</code></pre>
                <p>
                    A real padlock. No warning, no exception, no
                    <code>curl -k</code>. Your browser trusts it, your HTTP
                    client trusts it, your service worker registers.
                </p>
                <p>
                    What happened: <code>grove init</code> created a
                    certificate authority on your machine and added it to the
                    system trust store. <code>grove secure</code> issues a
                    certificate for that site, signed by it. Your system
                    trusts the CA, so it trusts the certificate. That is the
                    whole mechanism, and it is the same one the public web
                    uses &mdash; only the authority is yours.
                </p>

                <h2 id="safe">Why a CA on your laptop is safe</h2>
                <p>
                    A certificate authority your machine trusts is, in
                    principle, a dangerous thing: a CA can vouch for
                    <em>any</em> hostname. If Grove&#039;s CA could sign a
                    certificate for <code>google.com</code>, and your system
                    trusts Grove&#039;s CA, then anyone who got hold of that
                    key could impersonate Google to you.
                </p>
                <p>
                    So the CA carries a <strong>name constraint</strong>: it is
                    cryptographically limited to your TLD. A certificate it
                    signs for anything outside <code>.test</code> is rejected
                    by the verifier &mdash; not by Grove&#039;s politeness, but
                    by the same rules your browser applies to every certificate
                    on the internet. The worst a leaked key can do is
                    impersonate a name that only ever resolves to your own
                    machine.
                </p>
                <p>
                    That claim is not a promise in a document. It is a
                    regression test in Grove&#039;s own suite, run through the
                    same verifier browsers use: a Grove-signed certificate for
                    <code>google.com</code> is refused, the identical
                    certificate signed by an unconstrained CA is accepted
                    &mdash; proving the refusal is the constraint doing its job
                    and not an unrelated failure &mdash; and the certificates
                    Grove actually issues are accepted.
                </p>
                <div class="callout">
                    <strong>Secure everything, not just what needs it.</strong>
                    There is no cost per site and no performance difference
                    worth measuring, and a machine where some sites are HTTPS
                    and some are not is a machine where you will spend an
                    afternoon on a mixed-content bug that only exists locally.
                    <code>grove unsecure &lt;site&gt;</code> reverses it if you
                    ever need the plain version.
                </div>
        """,
        "learned": [
            "<strong>Service workers, secure cookies, OAuth callbacks and WebAuthn need HTTPS</strong> &mdash; local development without it tests something you do not ship.",
            "<strong>Click-through warnings train the wrong reflex</strong> and still do not enable the features you wanted.",
            "<strong>grove secure issues a certificate from a CA your system already trusts.</strong>",
            "<strong>The CA is name-constrained to your TLD,</strong> enforced by the verifier rather than by good manners.",
            "<strong>Secure every site.</strong> Mixed protocols across a machine cost more than the command does.",
        ],
        "next": "PHP: several versions at once, pinned per site, with the extensions you need &mdash; including the ones no prebuilt binary has.",
    },
    # ------------------------------------------------------------------ 5
    {
        "n": 5,
        "title": "PHP, Exactly the One You Need",
        "summary": "Several versions at once, pinned per site, and an honest account of which extensions are there and which are not.",
        "lead": """
            The client project needs 8.1, the new one needs 8.5, and one of
            them needs an extension no prebuilt binary ships. This chapter is
            about having all of that at the same time, and about a screen that
            tells you what is missing before your code does.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    PHP is usually global. One version on the machine, changed
                    by a command that affects everything, so the moment you
                    maintain two projects on different versions you are
                    switching several times a day &mdash; and eventually
                    running the wrong one without noticing, which produces
                    errors that make no sense because they are answers to a
                    question you did not ask.
                </p>
                <p>
                    Extensions make it worse. They are compiled against a
                    specific PHP, so every version change means rebuilding
                    them, and a missing one announces itself as
                    <code>Call to undefined function</code> in a stack trace
                    forty frames deep.
                </p>

                <h2 id="runtimes">Versions, installed and pinned</h2>
                <pre><code>grove php list
grove php install 8.3
grove use 8.3              # the machine default</code></pre>
                <img src="images/php-runtimes.png" alt="Grove&#039;s PHP panel: installed runtimes 8.5 and 8.4 with Update buttons, 8.3 with Install, and an extensions summary per runtime showing modules present and missing." />
                <p>
                    These are <strong>self-contained static PHP-FPM builds
                    downloaded into Grove</strong>. No Homebrew, no Herd, no
                    compilation, and no shared library on your system that a
                    later upgrade can pull out from under them.
                </p>
                <p>
                    The default is the machine&#039;s. The interesting command
                    is the per-site one:
                </p>
                <pre><code>grove isolate legacy-client 8.1</code></pre>
                <p>
                    Now <code>legacy-client.test</code> is served by 8.1 while
                    everything else stays on the default. Both run at the same
                    time; there is no switching, because there is nothing to
                    switch. Grove is the web server, so it knows which runtime
                    a request belongs to and routes to it &mdash; the thing
                    chapter 1 said one supervisor makes possible.
                    <code>grove unisolate</code> puts it back.
                </p>

                <h2 id="extensions">The extensions screen, and why it exists</h2>
                <p>
                    Look at the panel again. Under each runtime:
                </p>
                <pre><code>php@8.4    47 modules, 1 required missing, 6 recommended missing
           missing: mysqli</code></pre>
                <p>
                    That line is worth more than it looks. The alternative
                    &mdash; and the normal experience with every other PHP
                    distribution &mdash; is finding out from a fatal error in
                    a request, at which point you are debugging your
                    application for a problem that is in your environment.
                </p>
                <pre><code>grove php ext</code></pre>
                <p>
                    gives the full per-extension breakdown, and this is the
                    part that makes it useful: it says <strong>why each one
                    matters</strong>. Not a list of names to search for, but
                    what depends on it. An extension you have never heard of
                    and do not need reads as fine; one your framework assumes
                    reads as a problem, in the same list.
                </p>

                <h2 id="variants">When the extension is not in any build</h2>
                <p>
                    Sooner or later you need something the prebuilt runtimes
                    do not carry &mdash; an older <code>mysqli</code>, an
                    imaging library, something from PECL that a client&#039;s
                    decade-old codebase depends on. Two answers, and it is
                    worth knowing both exist before you need them.
                </p>
                <p>
                    <strong>Variants.</strong> Grove&#039;s PHP builds come in
                    more than one flavour, selected with
                    <code>--variant</code>, so an extension missing from one
                    may simply be present in another. Try this first; it is a
                    download rather than a project.
                </p>
                <p>
                    <strong>Register your own.</strong> If you already have a
                    PHP that does what you need &mdash; from Homebrew, from a
                    client&#039;s Docker image, compiled yourself &mdash; tell
                    Grove about it:
                </p>
                <pre><code>grove php register /opt/homebrew/opt/php@8.2/sbin/php-fpm</code></pre>
                <p>
                    Grove supervises it, routes to it and isolates sites to it
                    exactly as it does its own. This is the escape hatch that
                    keeps &ldquo;bring your own runtime&rdquo; from being a
                    reason not to adopt the rest, and it is the same mechanism
                    chapter 12 needs for step-debugging.
                </p>
                <div class="callout">
                    <strong>Check the extensions on a quiet afternoon,</strong>
                    not while something is failing. Five minutes with
                    <code>grove php ext</code> now is an hour you do not spend
                    later reading a stack trace that was never about your code.
                </div>
        """,
        "learned": [
            "<strong>Self-contained static builds</strong> &mdash; no Homebrew, nothing on your system to break them later.",
            "<strong>grove use sets the machine default; grove isolate pins one site.</strong> Both versions run at once, so there is no switching.",
            "<strong>The extensions panel tells you what is missing before your code does,</strong> and <code>grove php ext</code> says why each one matters.",
            "<strong>Variants first, register second.</strong> A missing extension may be a different flavour of the same build.",
            "<strong>Any PHP-FPM can be registered</strong> and is then supervised, routed and isolated like Grove&#039;s own.",
        ],
        "next": "the rest of the stack: databases, cache and a mail-catcher that Grove installs and supervises itself &mdash; and the .env block that wires your app to all of it.",
    },
    # ------------------------------------------------------------------ 6
    {
        "n": 6,
        "title": "The Rest of the Stack",
        "summary": "Databases, Redis and a mail-catcher Grove installs itself &mdash; and one command that writes the .env block for them.",
        "lead": """
            A database, a cache and somewhere for outgoing mail to land. This
            chapter is about not installing any of them yourself, and about
            the small command that ends the most tedious ten minutes of
            starting a project.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Backing services are where local environments rot. A
                    Homebrew MySQL upgraded itself during an unrelated
                    <code>brew upgrade</code> and now refuses to start on your
                    old data directory. A Docker Postgres eats eight gigabytes
                    of disk and a fan-spinning share of your battery for a
                    database with four tables in it. Redis was installed for a
                    project you finished last year and is still running.
                </p>
                <p>
                    Nothing is <em>wrong</em> with any of these tools. The
                    problem is that nothing owns them. They were installed by
                    different mechanisms at different times and no single
                    thing knows what is meant to be running, at which version,
                    on which port.
                </p>

                <h2 id="services">Services Grove owns</h2>
                <img src="images/services.png" alt="Grove&#039;s Services panel: dns, http, https and mail status pills, PostgreSQL, MySQL and ElyraSQL under Database, Redis under Cache &amp; Queue, with Install, Restart and Stop buttons and a Copy .env button." />
                <pre><code>grove service list
grove service install postgres
grove service start postgres</code></pre>
                <p>
                    Grove downloads and supervises PostgreSQL, MySQL,
                    ElyraSQL and Redis itself &mdash; the line under the panel
                    says it plainly: <em>no Homebrew, MySQL or Redis to
                    install separately</em>. They start when Grove starts,
                    stop when it stops, and <code>grove doctor</code> knows
                    whether they are healthy.
                </p>
                <p>
                    And because Grove owns them, it can do things a service
                    you installed yourself cannot &mdash; which is chapter 9,
                    and is the strongest argument in this chapter.
                </p>
                <p>
                    One of those things arrived in 1.9.0. A database you use
                    twice a week still costs its memory all week, so it can now
                    run <strong>only while something is connected</strong>:
                </p>
                <pre><code>grove service on-demand mysql on</code></pre>
                <p>
                    Grove holds the port. The server starts behind the first
                    connection and stops cleanly after ten idle minutes by
                    default. Measured on the machine that wrote this, an idle
                    MySQL at 517 MB went to nothing, and the first query after
                    it had stopped was answered in 0.35 s including the start.
                    It is opt-in: nothing about an existing service changes
                    until you turn it on.
                </p>

                <h2 id="env">The ten tedious minutes, ended</h2>
                <p>
                    Every new project starts the same way: open
                    <code>.env</code>, try to remember the port, get the
                    password wrong, find the mail settings from an old
                    project, run the migration, get a connection error, look
                    up what the socket path is this time.
                </p>
                <pre><code>grove env</code></pre>
                <pre><code>DB_CONNECTION=pgsql
DB_HOST=127.0.0.1
DB_PORT=5432
DB_DATABASE=grove
DB_USERNAME=grove
DB_PASSWORD=
REDIS_HOST=127.0.0.1
REDIS_PORT=6379
MAIL_MAILER=smtp
MAIL_HOST=127.0.0.1
MAIL_PORT=1025</code></pre>
                <p>
                    Paste it in. It is correct because Grove is the thing
                    running the services, so it is reading its own
                    configuration rather than reciting a convention. The GUI
                    has the same thing behind <strong>Copy .env</strong>.
                </p>

                <h2 id="mail">Mail that never leaves the building</h2>
                <p>
                    Note the last three lines. Grove runs a mail-catcher on
                    <code>127.0.0.1:1025</code>, and everything your apps send
                    is <strong>captured, never delivered</strong>.
                </p>
                <pre><code>grove mail</code></pre>
                <pre><code>#  FROM              TO               SUBJECT           RECEIVED
1  hello@freddy.test you@example.com  Welcome aboard!   12:04:31</code></pre>
                <p>
                    This is a safety feature more than a convenience. Every
                    developer who has been doing this a while knows somebody
                    who sent a test run of a welcome sequence to a production
                    user table &mdash; or was that somebody. With mail pointed
                    at 1025 from the first commit, the accident is
                    structurally impossible on this machine, and you also get
                    to read the HTML your mailer actually produced instead of
                    imagining it.
                </p>

                <h2 id="which-database">Which database</h2>
                <p>
                    Use whichever your production runs. Local development that
                    differs from production in the database is how you find
                    out about a dialect difference during a deploy.
                </p>
                <p>
                    <strong>ElyraSQL</strong> is worth a note since it is
                    newer: a MySQL-compatible server in one static binary,
                    with the whole database in a single file. It speaks the
                    MySQL protocol, so an app connects to it with
                    <code>DB_CONNECTION=mysql</code> and no code changes. The
                    single file is the interesting part &mdash; it makes the
                    snapshots in chapter 9 a copy of one file rather than a
                    dump and restore.
                </p>
                <div class="callout">
                    <strong>Coming from Herd?</strong> Grove&#039;s Tools
                    panel has <em>Migrate MySQL from Herd</em>: it copies every
                    database from another MySQL server into Grove&#039;s with a
                    logical dump and restore, and <strong>leaves the source
                    untouched</strong>. The one requirement is that the two
                    servers are on different ports &mdash; if both want 3306,
                    move Grove&#039;s to 3307 under Services first. Nothing is
                    deleted, so you can run both until you are convinced.
                </div>
        """,
        "learned": [
            "<strong>Grove installs and supervises its own databases, cache and mail</strong> &mdash; one thing owns them, at a known version, on a known port.",
            "<strong>grove env writes the .env block</strong> from what is actually running rather than from memory.",
            "<strong>Mail is captured, never delivered.</strong> The worst accident in local development becomes structurally impossible.",
            "<strong>Match production&#039;s database.</strong> A dialect difference found during a deploy is a bad way to learn one.",
            "<strong>Herd&#039;s databases can be copied over</strong> with the source left untouched, as long as the ports differ.",
        ],
        "next": "the terminal becomes Grove&#039;s: php and composer that follow the project you are standing in, and the moment Herd and Valet can come off.",
    },
    # ------------------------------------------------------------------ 7
    {
        "n": 7,
        "title": "The Terminal Follows the Project",
        "summary": "Shims that make php, composer and node resolve per project &mdash; and the chapter where Herd and Valet come off.",
        "lead": """
            Your sites are served by the right PHP. Your terminal is not: it
            still runs whatever is first on your PATH, which is how
            <code>php artisan</code> ends up on a different version from the
            site it belongs to. This chapter fixes that, and it is the point
            where the old tools can go.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    There are two PHPs in your life and they are not the same
                    one. The <strong>server</strong> PHP runs your site;
                    chapter 5 made that per-project. The
                    <strong>terminal</strong> PHP runs
                    <code>php artisan migrate</code>, <code>composer
                    install</code>, <code>vendor/bin/pest</code> &mdash; and it
                    is whatever your PATH finds first, globally, regardless of
                    which directory you are standing in.
                </p>
                <p>
                    The failure is quiet and confusing. A site served on 8.1
                    whose migrations run on 8.5 will mostly work, until a
                    dependency resolves differently, or a composer install
                    writes a lock file for the wrong platform, or a test
                    passes locally and fails in CI. The cause is never where
                    the symptom is.
                </p>

                <h2 id="shims">Shims</h2>
                <pre><code>grove path install</code></pre>
                <pre><code>✓ Installed shims for php, composer, cpx, node, npm, npx, laravel.
✓ provisioned toolchain: PHP 8.5 CLI, Composer, cpx, Node 22

    echo 'export PATH="$HOME/Library/Application Support/Grove/shims:$PATH"' &gt;&gt; ~/.zshrc</code></pre>
                <p>
                    Add the line, restart your shell, and the commands you type
                    are Grove&#039;s. A shim is a tiny program that asks one
                    question before it runs anything: <em>which project am I
                    in, and what does it pin?</em> Then it executes that
                    version.
                </p>
                <pre><code>cd ~/Code/legacy-client &amp;&amp; php -v      # PHP 8.1
cd ~/Code/freddy         &amp;&amp; php -v      # PHP 8.5</code></pre>
                <p>
                    No switching, no ceremony, no remembering. The same
                    mechanism covers <code>composer</code>, <code>node</code>,
                    <code>npm</code>, <code>npx</code> and the
                    <code>laravel</code> installer, so an npm install in a
                    project pinned to Node 20 uses Node 20.
                </p>

                <h2 id="cpx">cpx, while we are here</h2>
                <p>
                    Grove also installs <strong>cpx</strong>, a Composer
                    package executor &mdash; <code>npx</code> for PHP. It runs
                    a Composer package without installing it into your
                    project:
                </p>
                <pre><code>cpx friendsofphp/php-cs-fixer fix src/</code></pre>
                <p>
                    Useful for the tools you want occasionally and do not want
                    in <code>composer.json</code> forever, where they will
                    constrain your dependency resolution for years for the
                    sake of something you ran twice.
                </p>

                <h2 id="uninstall">Removing Herd and Valet</h2>
                <p>
                    This is the moment. Everything the old tool did is now
                    done: DNS, the proxy, certificates, PHP versions, the
                    database, the terminal toolchain. Three chapters of
                    evidence that the replacement works, rather than a leap.
                </p>
                <ol>
                    <li>
                        <strong>Copy the databases first</strong> if you have
                        not &mdash; chapter 6, with the source untouched.
                    </li>
                    <li>
                        <strong>Uninstall the old tool</strong> its own way.
                    </li>
                    <li>
                        <strong>Run <code>sudo grove install</code> once
                        more.</strong> Uninstalling Herd or Valet removes
                        resolver files and trust-store entries on the way out,
                        including, sometimes, ones that were not theirs. This
                        re-asserts Grove&#039;s resolver and CA. If
                        <code>.test</code> stops resolving after you remove
                        something, this is the command.
                    </li>
                    <li>
                        <strong><code>grove doctor</code></strong> to confirm
                        the ports, the resolver and the CA are all where they
                        should be.
                    </li>
                </ol>
                <div class="callout">
                    <strong>You can run both for a week.</strong> Nothing in
                    this course requires the old tool to be gone; the two can
                    coexist as long as they are not fighting over ports 80,
                    443 and 3306. Move when you stop reaching for the old one,
                    not before.
                </div>
        """,
        "learned": [
            "<strong>Two PHPs:</strong> the one serving the site and the one in your terminal. Only the first was per-project until now.",
            "<strong>Shims ask which project you are in</strong> before running php, composer, node, npm, npx or laravel.",
            "<strong>cpx runs a Composer package without installing it,</strong> keeping occasional tools out of composer.json forever.",
            "<strong>Remove the old tool after the evidence,</strong> not before &mdash; and copy the databases first.",
            "<strong>Run <code>sudo grove install</code> after uninstalling Herd or Valet.</strong> They take resolver and trust entries with them.",
        ],
        "next": "the processes a project needs while you work &mdash; queue workers, Vite, schedulers &mdash; started and supervised by the same thing that serves the site.",
    },
    # ------------------------------------------------------------------ 8
    {
        "n": 8,
        "title": "The Processes a Project Needs",
        "summary": "Queue workers, Vite and schedulers started and supervised together &mdash; and the one command you must not put in the list.",
        "lead": """
            A modern app is not one process. It is a site, a queue worker, an
            asset watcher and a scheduler, and the usual way to run them is
            four terminal tabs you forget to reopen. This chapter is about
            giving that job to the thing that is already supervising
            everything else.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    You know the four tabs. One for <code>npm run dev</code>,
                    one for <code>php artisan queue:work</code>, one for
                    <code>php artisan schedule:work</code>, one to actually
                    type in. They are unlabelled, they are in a different
                    order on Tuesday, and when you restart your machine you
                    remember three of them.
                </p>
                <p>
                    The bug that produces is specific and maddening: a feature
                    that works for you and not for a colleague, or this
                    morning and not this afternoon, because a queue worker was
                    running then and is not now. Nothing tells you. The job is
                    simply queued forever, and the email never arrives.
                </p>

                <h2 id="dev">grove dev</h2>
                <pre><code>grove dev freddy</code></pre>
                <p>
                    Grove starts the project&#039;s development processes and
                    supervises them alongside the site &mdash; same lifecycle,
                    same logs, same place to look. Stop the site and they stop
                    with it.
                </p>
                <p>
                    It reads what to run from the project itself, so a
                    colleague who clones the repository gets the same
                    processes without being told about them. That is the same
                    idea as chapter 13, arriving early: the environment is
                    described in the repository rather than in somebody&#039;s
                    habits.
                </p>

                <h2 id="the-trap">The one command that must not be in the list</h2>
                <p>
                    A Laravel project may define a <code>dev</code> script that
                    runs everything at once &mdash; typically through
                    <code>concurrently</code>, starting
                    <code>php artisan serve</code> among other things.
                </p>
                <p>
                    <strong>Do not let Grove run that one.</strong> It starts
                    PHP&#039;s built-in development server, which then serves
                    your site on a port, in parallel with Grove serving the
                    same site properly. Two servers, one project. The symptoms
                    are worth recognising because they look like a bug in your
                    application: a change appears on one reload and not the
                    next, sessions behave strangely, and a debugger attaches to
                    a process that is not handling the request you are looking
                    at.
                </p>
                <p>
                    Grove skips the entries it knows about &mdash; the
                    all-in-one <code>dev</code> script and the built-in server
                    &mdash; and says which it skipped, rather than quietly
                    doing something confusing. Run the individual processes:
                    the queue worker, the asset watcher, the scheduler. Let
                    Grove be the web server, since it already is one.
                </p>

                <h2 id="logs">Where it went wrong</h2>
                <pre><code>grove logs
grove logs freddy</code></pre>
                <p>
                    One place for the daemon&#039;s log, a site&#039;s PHP-FPM
                    log, and the output of the dev processes above. Which
                    matters most when a dev process has died: with four
                    terminal tabs, a crashed queue worker is a tab you are not
                    looking at, and the first evidence is a feature that
                    silently does not work. Supervised, it is a line in a log
                    you already know how to read.
                </p>
                <div class="callout">
                    <strong>The test for whether you have this right:</strong>
                    restart your machine, run one command, and have everything
                    the project needs running again. If the answer involves
                    remembering anything, the environment is still in your head
                    rather than in the repository.
                </div>
        """,
        "learned": [
            "<strong>An app is several processes,</strong> and forgotten ones fail silently &mdash; the job is queued forever and nobody is told.",
            "<strong>grove dev supervises them with the site,</strong> same lifecycle and same logs.",
            "<strong>Never let it run the all-in-one dev script</strong> &mdash; <code>artisan serve</code> beside Grove means two servers for one project.",
            "<strong>Grove skips the entries it knows about and says so,</strong> rather than quietly doing something confusing.",
            "<strong>One command after a restart.</strong> If it takes remembering, the environment is still in your head.",
        ],
        "next": "the thing only a supervisor that owns the database can offer: a snapshot you take before the scary migration and restore in one command.",
    },
    # ------------------------------------------------------------------ 9
    {
        "n": 9,
        "title": "Before the Scary Migration",
        "summary": "Snapshots and one-command restores &mdash; what it buys you, and why owning the database is what makes it possible.",
        "lead": """
            Every developer has run a migration they were not sure about and
            felt the small cold moment afterwards. This chapter removes it, in
            two commands, and explains why this particular feature could only
            come from the thing that installed your database.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    A destructive migration cannot be undone by
                    <code>migrate:rollback</code>. The <code>down()</code>
                    method restores the <em>schema</em>; it cannot restore the
                    column you dropped, because the data went with it. And in
                    local development the data is often worth more than it
                    sounds: three weeks of hand-made test cases covering the
                    edge conditions you had to reason about, which a fresh
                    seed does not contain.
                </p>
                <p>
                    So people do one of two things. They avoid the experiment
                    &mdash; writing the migration defensively, never trying the
                    cleaner schema &mdash; or they run it and lose an
                    afternoon rebuilding fixtures. Both are the same tax, paid
                    differently.
                </p>

                <h2 id="the-hard-way">The hard way</h2>
                <p>
                    You can do this yourself:
                    <code>mysqldump</code> to a file, remember where you put
                    it, remember the flags, restore with the inverse command,
                    remember which of the four dumps in your Downloads folder
                    was the right one. It works, and the number of people who
                    actually do it before every risky migration is
                    approximately zero, because the ceremony is longer than the
                    migration.
                </p>

                <h2 id="snapshots">Two commands</h2>
                <pre><code>grove db snapshot --db myapp --note "before migrate"
# ...run the scary migration...
grove db list
grove db restore &lt;id&gt;</code></pre>
                <p>
                    Data restored exactly as it was. The note is not
                    decoration &mdash; a week later, <code>grove db list</code>
                    with five snapshots in it is unreadable without one.
                </p>
                <p>
                    &ldquo;Exactly&rdquo; has been true since 1.9.0, and it is
                    worth knowing why it was not before. A MySQL dump recreates
                    the tables it holds and says nothing about any others, so a
                    restore after a migration that <em>created</em> a table
                    left that table in place &mdash; the one case a snapshot
                    before a migration exists for. Each database in the dump is
                    now dropped and recreated on the way in. If you are on an
                    older Grove, upgrade before you rely on this chapter.
                </p>
                <p>
                    Snapshots live under Grove&#039;s own directory rather than
                    in your project, which keeps a database dump out of your
                    repository and out of your build context. MySQL and
                    PostgreSQL snapshots are plain SQL dumps &mdash; readable,
                    greppable, restorable by hand if you ever want to. An
                    ElyraSQL snapshot is a hot, consistent copy of its single
                    database file, taken while it serves.
                </p>

                <h2 id="branches">A database per branch</h2>
                <p>
                    A snapshot answers &ldquo;put it back&rdquo;. There is a
                    second, quieter version of the same problem: you switch
                    from a feature branch to <code>main</code>, and the feature
                    branch&#039;s migrations are still in the database. Now
                    <code>main</code> is running against a schema it has never
                    seen, and the errors look like bugs in <code>main</code>.
                </p>
                <pre><code>grove db branches on</code></pre>
                <p>
                    Since 1.9.0 a project can have a database per git branch.
                    Check out a branch and its data is swapped in; go back to
                    <code>main</code> and <code>main</code>&#039;s data comes
                    back exactly as it was left. The first checkout of a new
                    branch starts it from the data you were on.
                </p>
                <p>
                    The design decision worth noticing: <strong>the database
                    keeps its name</strong>, and only its contents move. So the
                    app, <code>php artisan</code> in a terminal, a test run and
                    a database client all see the checked-out branch without
                    being told. Pointing the app at a different database per
                    branch would have reached only the requests Grove proxies,
                    and your terminal would have kept migrating the old one. On
                    MySQL the swap is a single atomic <code>RENAME TABLE</code>,
                    measured at 43&ndash;46 ms for a 70 MB schema.
                </p>

                <h2 id="why-possible">Why only a supervisor can do this</h2>
                <p>
                    This is the chapter that makes the argument for the whole
                    course, so it is worth stating directly.
                </p>
                <p>
                    A tool that merely <em>connects</em> to your database can
                    dump it. Only a tool that <em>owns</em> it knows which
                    engine is running, on which port, with which credentials,
                    for which project &mdash; without being told, every time,
                    by you. Grove installed the database, so
                    <code>grove db snapshot</code> needs no connection string:
                    it already knows.
                </p>
                <p>
                    That is the same property that made <code>grove env</code>
                    correct in chapter 6 and per-site PHP routing possible in
                    chapter 5. Each is small on its own. Together they are the
                    difference between a collection of tools and an
                    environment.
                </p>

                <h2 id="habit">Make it a habit</h2>
                <p>
                    Snapshot before a migration you have not run before.
                    Snapshot before running someone else&#039;s migration on
                    your data. Snapshot before <code>migrate:fresh --seed</code>
                    on a database you have been building up by hand.
                </p>
                <p>
                    It costs seconds and it changes what you are willing to
                    try &mdash; which is the real benefit. The best version of
                    a schema is usually the third one you attempt, and you only
                    get to the third if the first two were cheap.
                </p>
                <div class="callout">
                    <strong>This is also what makes an agent safe to let
                    near your database.</strong> If you have a coding agent
                    running migrations, a snapshot beforehand turns
                    &ldquo;what did it just do&rdquo; from a question into a
                    command. Chapter 14 comes back to this.
                </div>
        """,
        "learned": [
            "<strong>down() restores the schema, not the data.</strong> Hand-made local fixtures are worth more than they sound.",
            "<strong>Two commands, with a note,</strong> because five unlabelled snapshots are as good as none.",
            "<strong>Snapshots live in Grove&#039;s directory,</strong> not in your repository or your build context.",
            "<strong>Owning the database is what makes it one command</strong> &mdash; the same property behind grove env and per-site PHP.",
            "<strong>Cheap experiments change what you attempt.</strong> The best schema is usually the third one.",
        ],
        "next": "the thing you cannot build from separate tools: the full timeline of a request, the queries it caused, and a replay button.",
    },
    # ------------------------------------------------------------------ 10
    {
        "n": 10,
        "title": "Seeing the Request",
        "summary": "A timeline of every request, the queries it caused, and a replay button &mdash; from the process that served it.",
        "lead": """
            Grove is the web server, so it sees every request before your
            application does and after it finishes. This chapter is about what
            that makes possible, and it is the feature you will miss most if
            you ever go back.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Something went wrong in a request you were not watching. A
                    webhook arrived at three in the morning. A form submission
                    failed for one user. An AJAX call returned 500 and the
                    page swallowed it. In each case the request is gone: you
                    have a log line if you are lucky, and the rest is
                    reconstruction from memory and guesswork.
                </p>
                <p>
                    Worse, reproducing it means recreating the exact request
                    &mdash; the same headers, the same body, the same session
                    &mdash; which for anything involving a third party is an
                    afternoon of work before you have even started debugging.
                </p>

                <h2 id="the-hard-way">The hard way</h2>
                <p>
                    The usual tools each see one slice. Your framework&#039;s
                    debug bar sees requests that rendered a page &mdash; not
                    the API call, not the webhook, not the one that died before
                    the view. The access log sees a line with a status code and
                    no body. Xdebug sees everything but only while you sit
                    there with a breakpoint, which is no use for something that
                    happened at three in the morning.
                </p>
                <p>
                    None of them can replay. And none of them can see the whole
                    request, because none of them is the web server.
                </p>

                <h2 id="timeline">The timeline</h2>
                <pre><code>grove requests
grove requests --site freddy</code></pre>
                <p>
                    Every request Grove served: method, path, status, duration.
                    Not a sample, not only the ones that rendered, not only the
                    ones your framework knew about &mdash; every one, because
                    Grove handled it.
                </p>
                <p>
                    Open one and you get its detail: headers, body, response,
                    timing. The request that failed at three in the morning is
                    there, with the payload that caused it.
                </p>

                <h2 id="replay">Replay, and copy as</h2>
                <p>
                    This is the part that changes how you debug. A captured
                    request can be <strong>replayed</strong> &mdash; sent again,
                    exactly as it arrived &mdash; so the loop becomes: replay,
                    read the error, change the code, replay. Seconds per
                    iteration, with no third party involved and no form to fill
                    in again.
                </p>
                <p>
                    And it can be copied out:
                </p>
                <ul>
                    <li>
                        <strong>as curl</strong> &mdash; to paste into a
                        terminal, or to a colleague who does not have your
                        machine.
                    </li>
                    <li>
                        <strong>as an HTTP file</strong> &mdash; for the
                        client in your editor.
                    </li>
                    <li>
                        <strong>as a Pest test</strong> &mdash; and this is
                        the one to notice. A bug you just reproduced becomes a
                        failing test in one click, which is the difference
                        between fixing a bug and fixing it <em>permanently</em>.
                        The barrier to writing that test was never the typing;
                        it was reconstructing the request.
                    </li>
                </ul>

                <h2 id="causal">The queries a request caused</h2>
                <p>
                    With SQL capture on, the timeline shows the database
                    queries belonging to each request &mdash; not a global
                    query log you must correlate by timestamp, but the queries
                    <em>that request</em> caused, in order, with their
                    durations.
                </p>
                <p>
                    Which makes the N+1 problem visible rather than deducible.
                    A page that runs 340 queries where you expected six is
                    obvious in a list; it is nearly invisible in a page that
                    renders in 900ms on a laptop with a warm local database,
                    right up until it is a production incident on a table with
                    a million rows.
                </p>
                <pre><code>grove explain</code></pre>
                <p>
                    walks the causal chain for a request in one place: what
                    arrived, what ran, what it asked the database, what came
                    back. The value is that it is one place. Every part of that
                    story exists somewhere in a conventional setup, in four
                    different tools with four different clocks.
                </p>
                <div class="callout">
                    <strong>Leave it on.</strong> The cost is small and the
                    benefit is retrospective: the request you wish you had
                    captured is always one that has already happened. A tool
                    you must remember to start is a tool that is off when you
                    need it.
                </div>
        """,
        "learned": [
            "<strong>Grove sees every request</strong> &mdash; including the ones that died before your framework was involved.",
            "<strong>Replay turns debugging into a loop</strong> measured in seconds, with no third party and no form to refill.",
            "<strong>Copy as Pest makes the fix permanent.</strong> The barrier to that test was reconstructing the request, not typing it.",
            "<strong>Queries are attributed to the request that caused them,</strong> which makes N+1 visible instead of deducible.",
            "<strong>Leave capture on.</strong> The request you wish you had is always one that already happened.",
        ],
        "next": "the outside world: a public URL for your local site, and webhooks you can re-deliver instead of re-triggering.",
    },
    # ------------------------------------------------------------------ 11
    {
        "n": 11,
        "title": "Letting the Outside In",
        "summary": "A public URL for a local site, and a webhook you can deliver again without going back to Stripe.",
        "lead": """
            Some things cannot be tested on localhost: a payment provider&#039;s
            webhook, an OAuth callback, a client who wants to see it. This
            chapter is about opening a door, and about not having to knock
            twice.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Third-party integrations are the worst part of local
                    development, and the reason is a loop that cannot close.
                    Stripe needs to send a webhook to a URL it can reach.
                    Your machine is not reachable. So you either deploy to a
                    staging environment to test each change &mdash; minutes per
                    iteration &mdash; or you fake the payload and discover on
                    launch day that the real one has a field you did not
                    expect.
                </p>

                <h2 id="tunnel">A public URL</h2>
                <pre><code>grove share freddy</code></pre>
                <p>
                    A public HTTPS address that forwards to your local site.
                    The provider can reach it, the client can open it on their
                    phone, the OAuth callback resolves. Your code stays on your
                    machine with your debugger attached and your breakpoints
                    in place.
                </p>
                <p>
                    Two things to be deliberate about. It is a door into your
                    laptop: share the site you are working on, close it when
                    you are done. And your app must produce correct absolute
                    URLs when reached by another name &mdash; the usual cause
                    of a share that loads but whose assets and redirects point
                    back at <code>.test</code>.
                </p>

                <h2 id="webhooks">The webhooks bucket</h2>
                <p>
                    Sharing solves reachability. It does not solve the
                    <em>second</em> problem, which is that every test costs a
                    real event: another payment, another push, another form
                    filled in on a provider&#039;s dashboard.
                </p>
                <pre><code>grove hooks</code></pre>
                <p>
                    Grove captures incoming webhooks in a bucket you can
                    inspect and, crucially, <strong>deliver again</strong>. So
                    the loop becomes:
                </p>
                <ol>
                    <li>Trigger the event once, for real.</li>
                    <li>Read the payload &mdash; the actual one, with the
                        fields the provider actually sends, not the one in
                        their documentation.</li>
                    <li>Deliver it to your app. Watch it fail.</li>
                    <li>Fix the code. Deliver the same payload again.</li>
                    <li>Repeat until it passes.</li>
                </ol>
                <p>
                    One real event, twenty iterations. The alternative is
                    twenty real events, which for a payment provider means
                    twenty test charges and a great deal of clicking.
                </p>
                <p>
                    And because the payload is the real one, you are debugging
                    against what the provider sends rather than against what
                    you believe they send. The gap between those two is where
                    integration bugs live.
                </p>
                <div class="callout">
                    <strong>Keep the interesting ones.</strong> A webhook that
                    exposed a bug is a fixture. Combined with chapter 10&#039;s
                    <em>copy as Pest</em>, the payload that broke your handler
                    in March becomes a test that stops it breaking again in
                    November &mdash; without an account at the provider or a
                    network connection.
                </div>
        """,
        "learned": [
            "<strong>grove share gives a local site a public HTTPS URL,</strong> so the provider can reach you while the code stays here.",
            "<strong>Close the door when you are done,</strong> and make sure your app builds absolute URLs from the request.",
            "<strong>Webhooks are captured and re-deliverable.</strong> One real event, twenty iterations.",
            "<strong>Debug against the payload they actually send,</strong> not the one in the documentation.",
            "<strong>A payload that found a bug is a fixture.</strong> Turn it into a test and it never comes back.",
        ],
        "next": "step-debugging: a breakpoint instead of a dump, why a static PHP cannot load Xdebug, and what to do about it.",
    },
    # ------------------------------------------------------------------ 12
    {
        "n": 12,
        "title": "A Breakpoint Instead of a Dump",
        "summary": "Xdebug on demand, the honest reason static builds cannot load it, and how to get it working anyway.",
        "lead": """
            Most PHP developers debug by printing things. Step-debugging is
            better in every way and almost nobody sets it up, because setting
            it up used to mean choosing between a debugger and a fast machine.
            That trade is gone; this chapter is the five minutes.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    <code>dd($user)</code> tells you one value at one point.
                    If the value is wrong you learn nothing about
                    <em>why</em>, so you move the dump up a frame and run it
                    again. And again. Each iteration is a page reload and a
                    guess about where to look, and the guessing is the slow
                    part.
                </p>
                <p>
                    A step-debugger answers a different question. Stop here;
                    now show me every variable in scope, the call stack that
                    got here, and let me step forward one line at a time. You
                    are not guessing where the value went wrong &mdash; you are
                    watching it.
                </p>

                <h2 id="the-hard-way">The hard way, and why nobody does it</h2>
                <p>
                    Historically, enabling Xdebug meant loading it for
                    <em>every</em> request, which made every request slower
                    &mdash; noticeably, on a large framework. So people enabled
                    it for an afternoon, felt their machine turn to treacle,
                    and turned it off. The tool that would have saved them
                    hours was a tool they associated with being slow.
                </p>

                <h2 id="trigger">On demand</h2>
                <img src="images/tools.png" alt="Grove&#039;s Tools panel: an Xdebug step-debugging toggle with per-runtime status, and a Migrate MySQL from Herd form with source host, port, user and password." />
                <pre><code>grove debug on</code></pre>
                <p>
                    Xdebug is loaded into PHP-FPM, but it only <em>activates</em>
                    for a request that opts in with an
                    <code>XDEBUG_TRIGGER</code> cookie or parameter. Your
                    editor listens on DBGp port 9003. Requests that do not opt
                    in pay almost no overhead.
                </p>
                <p>
                    So the debugger is always available and never in the way.
                    A browser extension sets the cookie with one click; the
                    page you are debugging stops at your breakpoint and every
                    other page on your machine runs at full speed.
                </p>

                <h2 id="static">The honest catch</h2>
                <p>
                    Read the panel: <em>unavailable &mdash; needs a PHP with
                    Xdebug</em>.
                </p>
                <p>
                    Grove&#039;s own PHP builds are fully static, and a fully
                    static binary cannot load a dynamic extension at runtime
                    &mdash; that is what static means. The same property that
                    makes those builds immune to a Homebrew upgrade breaking
                    them is the property that stops them loading Xdebug. It is
                    a real trade-off and the panel says so rather than leaving
                    you to work it out from an error.
                </p>
                <p>
                    The fix is chapter 5&#039;s escape hatch:
                </p>
                <pre><code>grove php register /opt/homebrew/opt/php@8.4/sbin/php-fpm</code></pre>
                <p>
                    Register a PHP that has Xdebug &mdash; from Homebrew, or
                    any build you like &mdash; and isolate the site you are
                    debugging to it. The rest of your sites keep the static
                    builds. You pay the trade only where you need it, and only
                    while you need it.
                </p>

                <h2 id="cli">Debugging the command line</h2>
                <p>
                    Breakpoints in an <code>artisan</code> command, a queue
                    job, or a failing test:
                </p>
                <pre><code>eval "$(grove debug env)"
php artisan queue:work</code></pre>
                <p>
                    That exports the environment Xdebug needs for CLI
                    processes in the current shell. Point your editor&#039;s
                    DBGp listener at port 9003 and your breakpoint in a job
                    handler is hit when the worker picks up the job.
                </p>
                <p>
                    Debugging a queue job is where step-debugging earns its
                    keep most decisively, because it is the place dumping is
                    worst: the output goes to a worker&#039;s log you are not
                    watching, the job may be retried, and the state you care
                    about is the payload it was given rather than anything you
                    typed.
                </p>
                <div class="callout">
                    <strong>Set it up before you need it.</strong> Nobody
                    configures a debugger while hunting a bug &mdash; the
                    pressure pushes you back to <code>dd()</code>, and the bug
                    that would have taken ten minutes takes two hours. Do it
                    now, on a quiet afternoon, and hit a breakpoint once so you
                    know it works.
                </div>
        """,
        "learned": [
            "<strong>A dump answers one question; a debugger answers the one you actually have.</strong>",
            "<strong>Trigger mode means always available, never in the way</strong> &mdash; requests that do not opt in pay almost nothing.",
            "<strong>Static builds cannot load Xdebug,</strong> which is the same property that makes them unbreakable. The panel says so.",
            "<strong>Register a PHP that has it</strong> and isolate only the site you are debugging.",
            "<strong>eval \"$(grove debug env)\" covers CLI</strong> &mdash; artisan commands, queue jobs, failing tests.",
            "<strong>Set it up on a quiet afternoon.</strong> Under pressure you will reach for dd() instead.",
        ],
        "next": "the chapter that makes all of this portable: one file in the repository that turns a git clone into a running environment.",
    },
    # ------------------------------------------------------------------ 13
    {
        "n": 13,
        "title": "Reproducible, in the Repository",
        "summary": "grove.toml: the environment described where the code lives, so a clone becomes a running setup with one command.",
        "lead": """
            Everything so far has been done to <em>your</em> machine. This
            chapter moves it into the repository, where it belongs &mdash; and
            it is the difference between a good personal setup and a good
            team.
        """,
        "body": """
                <h2 id="the-problem">The problem</h2>
                <p>
                    Your machine is now excellent, and entirely undocumented.
                    The PHP version this project needs lives in Grove&#039;s
                    state; the services it needs live in your memory; the dev
                    processes live in your habits.
                </p>
                <p>
                    Which produces the oldest problem in software: the new
                    colleague&#039;s first two days. A README written once,
                    drifted since, and a sequence of questions nobody enjoys
                    on either side. And the quieter version of the same
                    problem &mdash; you, on a new laptop, or you in eight
                    months on a project you have not opened since.
                </p>

                <h2 id="the-hard-way">Why the README does not work</h2>
                <p>
                    A README is prose describing state, and the two drift apart
                    immediately, in one direction: the setup changes and the
                    document does not. Nothing breaks when the README goes
                    stale, so nothing tells you it has &mdash; the only
                    detector is a new person failing to follow it, which
                    happens rarely and produces embarrassment rather than a
                    fix.
                </p>
                <p>
                    Setup instructions in prose are also not
                    <em>checkable</em>. &ldquo;Install PHP 8.3&rdquo; cannot be
                    verified by anything; it can only be read and obeyed or
                    misread and not.
                </p>

                <h2 id="grove-toml">One file, committed</h2>
                <pre><code>grove up --write     # scaffold a starter grove.toml
grove up             # link + pin PHP/Node + start services + optional dev</code></pre>
                <pre><code># grove.toml
name = "freddy"
php = "8.5"
services = ["mysql", "redis"]
dev = true</code></pre>
                <p>
                    Commit it. Now the environment is described in the place
                    that cannot drift from the code, because it is versioned
                    <em>with</em> the code: the commit that requires PHP 8.5
                    is the commit that says so.
                </p>
                <p>
                    A teammate clones the repository and runs one command:
                </p>
                <pre><code>git clone git@github.com:you/freddy.git
cd freddy
grove up</code></pre>
                <pre><code>Bringing up freddy…
  ✓ link
  ✓ https on
  ✓ php 8.5
  ✓ mysql
  ✓ redis
  ✓ dev
✓ freddy is up → https://freddy.test</code></pre>
                <p>
                    Linked, secured, pinned, services running, dev processes
                    started. From clone to working in one command, on a machine
                    that has never seen this project.
                </p>

                <h2 id="what-it-fixes">What this actually fixes</h2>
                <ul>
                    <li>
                        <strong>Onboarding</strong> stops being a ritual. The
                        first day is spent reading code rather than installing
                        things.
                    </li>
                    <li>
                        <strong>&ldquo;Works on my machine&rdquo;</strong>
                        gets narrower. The runtime and the services are the
                        same by construction, so when behaviour differs the
                        cause is somewhere more interesting.
                    </li>
                    <li>
                        <strong>Old projects come back to life.</strong> The
                        client project you last touched in March opens without
                        archaeology, because the file remembers what you do
                        not.
                    </li>
                    <li>
                        <strong>Upgrades become reviewable.</strong> Moving to
                        PHP 8.5 is a one-line diff in a pull request, discussed
                        and merged like any other change, rather than a message
                        in Slack asking everyone to please switch.
                    </li>
                </ul>

                <h2 id="bundles">When one command is still too many</h2>
                <p>
                    Grove can also produce a <strong>bundle</strong>: a
                    reproducible snapshot of an environment. The distinction
                    is worth holding on to &mdash; <code>grove.toml</code>
                    describes what to build, a bundle captures what was built.
                    The first is what you commit and live with; the second is
                    for handing an exact environment to someone, or to
                    yourself later, without rebuilding it from its
                    description.
                </p>
                <div class="callout">
                    <strong>Add grove.toml to every project you own,</strong>
                    including the ones with no colleagues. The person it most
                    often helps is you, on the day you open something you have
                    not touched in a year, at which point you are effectively a
                    new team member on your own project.
                </div>
        """,
        "learned": [
            "<strong>A README is prose describing state,</strong> and nothing breaks when it goes stale &mdash; so nothing tells you.",
            "<strong>grove.toml is versioned with the code,</strong> so the commit that needs PHP 8.5 is the commit that says so.",
            "<strong>One command from clone to running,</strong> on a machine that has never seen the project.",
            "<strong>Upgrades become a reviewable one-line diff</strong> instead of a message asking everyone to switch.",
            "<strong>grove.toml describes what to build; a bundle captures what was built.</strong>",
            "<strong>Commit it even alone.</strong> In a year you are a new team member on your own project.",
        ],
        "next": "the last chapter: shared secrets that the server cannot read, an AI assistant that can see your environment, and the rhythm that keeps a good setup good.",
    },
    # ------------------------------------------------------------------ 14
    {
        "n": 14,
        "title": "The Team, and Keeping It",
        "summary": "Secrets shared end-to-end encrypted, an assistant that can see your environment, and the habits that stop it rotting.",
        "lead": """
            Two things left that a team needs and a single developer does not,
            one thing that changes how an assistant can help you, and the
            short list of habits that keep the whole setup healthy.
        """,
        "body": """
                <h2 id="secrets">The last thing sent over Slack</h2>
                <p>
                    <code>grove.toml</code> describes the environment, and it
                    is committed, which is exactly why it cannot hold the
                    Stripe test key or the S3 credentials. So those travel the
                    old way: pasted into a chat message, and living there
                    forever in a searchable archive belonging to a company that
                    is not yours, readable by everyone who joins the channel
                    afterwards.
                </p>
                <p>
                    <strong>Grove Teams</strong> syncs a project&#039;s secrets
                    between team members with end-to-end encryption. The
                    claim worth understanding is the boundary: encryption and
                    decryption happen on your machines, so
                    <strong>the backend stores ciphertext it cannot
                    read</strong>. It is a courier, not a keyholder.
                </p>
                <p>
                    That is a specific and checkable claim, and it is the right
                    one to ask of anything holding your credentials. The docs
                    state exactly how much the backend is trusted rather than
                    saying &ldquo;secure&rdquo; and moving on &mdash; read that
                    section before you adopt it, because a boundary you have
                    not read is a boundary you are assuming.
                </p>

                <h2 id="mcp">An assistant that can see the environment</h2>
                <p>
                    Grove exposes an <strong>MCP server</strong>, so an AI
                    assistant &mdash; Elyra, Claude, Cursor &mdash; can see
                    your sites, requests, logs and database schema.
                </p>
                <p>
                    The difference this makes is worth being precise about. An
                    assistant without it is reasoning from your code and your
                    description of the problem. With it, the questions change:
                </p>
                <ul>
                    <li>
                        <em>&ldquo;Why is this endpoint slow?&rdquo;</em>
                        &mdash; it can read the actual request timeline and the
                        queries that request made, rather than guessing from
                        the controller.
                    </li>
                    <li>
                        <em>&ldquo;Write a query for this.&rdquo;</em> &mdash;
                        it can read the live schema rather than inferring it
                        from migrations, which are history, and models, which
                        are intent.
                    </li>
                    <li>
                        <em>&ldquo;What broke?&rdquo;</em> &mdash; it can read
                        the log instead of asking you to paste it.
                    </li>
                </ul>
                <p>
                    Combine it with chapter 9: a snapshot before you let an
                    agent run migrations turns &ldquo;what did it just
                    do&rdquo; from a question into a command.
                </p>

                <h2 id="rhythm">Keeping it healthy</h2>
                <p>
                    A good setup decays the same way a good codebase does
                    &mdash; slowly, invisibly, one unexamined change at a time.
                    Four habits, none of them longer than a minute:
                </p>
                <ul>
                    <li>
                        <strong><code>grove doctor</code> when something is
                        odd</strong>, before you start guessing. It checks the
                        things that are true or false rather than matters of
                        opinion, and it is faster than any theory you have.
                    </li>
                    <li>
                        <strong><code>grove db snapshot</code> before anything
                        irreversible.</strong> Seconds, and it changes what you
                        are willing to try.
                    </li>
                    <li>
                        <strong>Keep <code>grove.toml</code> honest.</strong>
                        When the project starts needing Redis, add it in the
                        same pull request. A file that drifts is a README with
                        better syntax.
                    </li>
                    <li>
                        <strong>Update deliberately.</strong> Grove updates its
                        binary without rewriting the service unit, so
                        <code>sudo grove install</code> after a major upgrade
                        is the visible step that adopts anything new at the
                        privileged layer. Since 1.9.0 it is not optional after
                        coming from before 1.8.0: the daemon will not run as
                        root, and it says so at startup.
                    </li>
                </ul>

                <h2 id="finished">What the perfect setup actually is</h2>
                <p>
                    Not a particular list of tools. It is a setup with three
                    properties, and every chapter here was in service of one
                    of them:
                </p>
                <p>
                    <strong>It is legible.</strong> You can say what is running
                    and why, and when something breaks there is a command that
                    tells you rather than a folklore you half-remember.
                </p>
                <p>
                    <strong>It is reproducible.</strong> It exists in the
                    repository, not in your head, so a colleague or a future
                    you can have it in one command.
                </p>
                <p>
                    <strong>It gets out of the way.</strong> The point was
                    never the environment. It was the hours you get back
                    because the environment stopped being a thing you think
                    about &mdash; which is exactly how much attention a
                    development environment deserves, and no more.
                </p>
        """,
        "learned": [
            "<strong>Secrets sync end-to-end encrypted;</strong> the backend stores ciphertext it cannot read &mdash; a courier, not a keyholder.",
            "<strong>Read the trust boundary before adopting it.</strong> A boundary you have not read is one you are assuming.",
            "<strong>MCP lets an assistant read the live environment</strong> &mdash; real timelines, the live schema, the actual log.",
            "<strong>Four habits:</strong> doctor before guessing, snapshot before anything irreversible, keep grove.toml honest, update deliberately.",
            "<strong>A perfect setup is legible, reproducible, and out of the way.</strong> The point was never the environment.",
        ],
        "next": "<strong>That is the course.</strong> The reference for everything here is the <a href=\"install.html\">installation guide</a> and the <a href=\"commands.html\">command reference</a>; the trust boundaries are in <a href=\"architecture.html\">architecture</a>. If your machine still has Herd or Valet on it, <a href=\"migrate-from-herd.html\">migrating</a> takes a few minutes and leaves your databases untouched.",
    },
]
