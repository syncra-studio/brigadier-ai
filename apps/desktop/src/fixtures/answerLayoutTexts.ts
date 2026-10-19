/**
 * The answers the answer-parts fixture and tests draw (Q86): the trading question answered with
 * its parts, a factual answer, a how-to, a free-form answer and a coding reply with none.
 * `PAGE` in an answer stands for its report's page id.
 */

export const TRADING_QUESTION = "How can i start trading, i need you to help me and give me the best options for me and who can help me";

/** The trading answer with every part: bottom line, steps, a pick, an avoid-list, odds, folds, Ask next. */
export const TRADING = [
  "Don't trade for quick money: with $500–1,000 it can't pay you an income, and most beginners who trade often lose [6]. If you can leave the money alone for 5 years or more, put it slowly into one low-cost index fund on Trading 212. If you'll need it sooner, keep it in the bank.",
  "",
  "1. Practise first. Open the free Trading 212 practice account (it gives you $50,000 of pretend money) and try it for 2–4 weeks [5].",
  "2. Open the real account. Choose **Invest**, not \"CFD\": CFDs are bets on prices, not real shares.",
  "3. Add a little money. A bank transfer is free; start with $50–100 and keep the rest in the bank for now.",
  "4. Buy one broad index fund. It is one purchase that holds hundreds of companies. Add a little each month and leave it for years.",
  "5. Keep every statement. Declare your profit on form CET18 by 30 April of the next year [9].",
  "",
  "### Which broker",
  "Both accept people who live in Moldova [1, 2].",
  "",
  "| Broker | Cost to buy | Practice money | Ease | Best for |",
  "|---|---|---|---|---|",
  "| Trading 212 Invest ★ | Free [3] | Yes | Easy | Starting small |",
  "| Interactive Brokers | $0.35–$1 each [4] | Yes | Harder | Larger amounts later |",
  "",
  "> [!CAUTION]",
  "> Avoid these",
  "> - Forex, CFDs and day trading: 7 to 9 out of every 10 small CFD accounts lose money [6].",
  "> - eToro and Lightyear (they don't serve Moldova), and XTB (only a weak Belize licence here).",
  "> - Crypto with this money: crypto services are banned in Moldova [7].",
  "> - Anyone who contacts you first, promises profit, or asks you to install AnyDesk. Check any company with the CNPF first: 022 85 95 95.",
  "",
  "---",
  "",
  "### Why not fast income",
  "```chance",
  "80 of 100",
  "small CFD accounts lose money",
  "```",
  "",
  "At a normal 5–8% a year, $1,000 earns about $50–80 a year. Earning $100 a month would mean making 10% every month, which almost nobody does. Over the long run, stocks have grown about 8.5% a year, with bad years along the way.",
  "",
  "### Taxes in Moldova",
  "- Profit when you sell: half of it is taxed at 12%, so about 6% of the profit [9].",
  "- Dividends from foreign companies: 12% [10].",
  "- Declare both yourself on form CET18 by 30 April of the next year.",
  "- I'm not fully sure of the dividend rate: one source says 6%, but the Tax Service's own guidance says 12%, so I used 12%.",
  "",
  "### How to spot a scam",
  "On 9 October 2026 Moldovan police raided five call centres running fake trading platforms [12]. Real brokers never phone you first, never promise profit, and never ask you to install AnyDesk or TeamViewer.",
  "",
  "### Who can help, for free",
  "- Is a company or \"adviser\" real? Ask the CNPF, Moldova's market regulator: 022 85 95 95, office@cnpf.md. It keeps a warning list [8].",
  "- Tax questions: the State Tax Service's free line, 0 8000 1525 [9].",
  "- Learning the basics: Investor.gov [11] and the practice account.",
  "",
  "### What I couldn't check",
  "Nobody tested a real deposit from a Moldovan bank card. At sign-up, check that Trading 212 offers you the Invest account. Everything the researchers found is in [Full research](brigadier-page:PAGE).",
  "",
  "### Ask next",
  "- Walk me through the practice account",
  "- How much should I put in each month?",
  "- What is an index fund?",
  "",
  "### Sources",
  "1. [Trading 212 — countries we serve](https://helpcentre.trading212.com/hc/en-us/articles/countries)",
  "2. [Interactive Brokers — open an account](https://www.interactivebrokers.com/en/accounts/open-account.php)",
  "3. [Trading 212 Invest — fees](https://www.trading212.com/terms/invest)",
  "4. [Interactive Brokers — stock commissions](https://www.interactivebrokers.com/en/pricing/commissions-stocks.php)",
  "5. [Trading 212 — practice account](https://www.trading212.com/practice-account)",
  "6. [ESMA — CFD losses of retail clients](https://www.esma.europa.eu/press-news/esma-news/esma-agrees-prohibit-binary-options-and-restrict-cfds-protect-retail-investors)",
  "7. [National Bank of Moldova — crypto services](https://www.bnm.md/en)",
  "8. [CNPF — warning list](https://www.cnpf.md/)",
  "9. [State Tax Service — income from investments](https://sfs.md/)",
  "10. [State Tax Service — foreign dividends](https://sfs.md/ro/dividende)",
  "11. [Investor.gov — introduction to investing](https://www.investor.gov/introduction-investing)",
  "12. [Moldovan police — fake trading call centres](https://politia.md/)",
].join("\n");

export const FACT_QUESTION = "When do I have to declare my investment income?";

/** A factual answer: the answer itself, a line after it and its source. No parts. */
export const FACT = [
  "By 30 April of the year after you earned it [1].",
  "",
  "You declare it yourself on form CET18, online at servicii.fisc.md or at any tax office. Your broker's yearly statement has the totals you need.",
  "",
  "### Sources",
  "1. [State Tax Service — CET18 declaration](https://sfs.md/)",
].join("\n");

export const HOWTO_QUESTION = "how do i open the trading 212 practice account?";

/** A how-to: its bottom line and steps, and nothing else. */
export const HOWTO = [
  "It takes about 5 minutes and needs only an email address — no money and no ID [1].",
  "",
  "1. Go to trading212.com or open the app, and tap **Practice**.",
  "2. Enter your email and choose a password.",
  "3. Pick **Invest** (not CFD) when it asks which account.",
  "4. You now have $50,000 of pretend money. Try buying a small piece of an index fund.",
  "",
  "When you're ready for real money, the same login opens the real account.",
  "",
  "### Sources",
  "1. [Trading 212 — practice account](https://www.trading212.com/practice-account)",
].join("\n");

export const FREE_QUESTION = "What's the difference between an ETF and a mutual fund?";

/** A free-form answer that uses no part: a paragraph, a bullet list, a table and a heading. */
export const FREE = [
  "Both pool many investors' money into one basket of stocks or bonds; the difference is how you buy and sell them.",
  "",
  "- An ETF trades on the stock exchange all day, like a share, at its current price.",
  "- A mutual fund is bought from the fund company once a day, at the price set after the market closes.",
  "- ETFs usually cost less to hold, and many brokers sell them with no fee.",
  "",
  "| | ETF | Mutual fund |",
  "|---|---|---|",
  "| When it trades | All day | Once a day |",
  "| Typical yearly cost | 0.03–0.2% | 0.5–1% |",
  "",
  "### Which to pick",
  "For a small monthly amount on Trading 212, an ETF: it is what the broker sells.",
].join("\n");

export const CODING_QUESTION = "Fix the login timeout";

/** A coding reply: a paragraph, then a numbered list, a table and a stray `---`. */
export const CODING = [
  "The login timed out because the session cookie expired before the refresh ran; it now refreshes a minute early.",
  "",
  "1. `auth/session.ts` refreshes at 59 minutes instead of 60.",
  "2. `auth/refresh.ts` retries once on a network error.",
  "3. `auth/session.test.ts` covers both.",
  "",
  "| File | Lines |",
  "|---|---|",
  "| auth/session.ts | 4 |",
  "",
  "---",
  "",
  "### Details",
  "Checks: pnpm test auth, passed. Review: none for a tiny edit. Not checked: nothing.",
].join("\n");
