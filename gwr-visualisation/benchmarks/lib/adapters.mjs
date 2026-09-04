// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

/* global wasm_bindgen */

import { mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { chromium } from "playwright-core";
import webdriver from "selenium-webdriver";
import safari from "selenium-webdriver/safari.js";

import { serveReport } from "./report-server.mjs";

const { Builder } = webdriver;

export function browserAdapters(config) {
  return config.browsers.map((name) => {
    if (name === "chromium") {
      return new ChromiumAdapter(config.chromiumExecutable);
    }
    if (name === "safari") {
      if (os.platform() !== "darwin") {
        throw new Error(
          "Safari browser tests require macOS and the system safaridriver",
        );
      }
      return new SafariAdapter();
    }
    throw new Error(`Unsupported browser '${name}'; use chromium or safari`);
  });
}

class ChromiumAdapter {
  constructor(executable) {
    this.name = "chromium";
    this.executable = executable;
    this.version = null;
  }

  async withSession(
    url,
    action,
    {
      allowStartupError = false,
      waitForSummary = false,
      failureEvidence = null,
    } = {},
  ) {
    const browser = await chromium.launch({
      executablePath: this.executable,
      headless: true,
      args: ["--allow-file-access-from-files"],
    });
    this.version ||= browser.version();
    try {
      const page = await browser.newPage({
        viewport: { width: 1440, height: 1000 },
      });
      const session = new ChromiumSession(page);
      try {
        await session.navigate(url, allowStartupError, waitForSummary);
        return await action(session);
      } catch (error) {
        if (failureEvidence) {
          await captureFailure(session, failureEvidence);
        }
        throw error;
      }
    } finally {
      await browser.close();
    }
  }
}

class ChromiumSession {
  constructor(page) {
    this.page = page;
    this.reloadGeneration = 0;
    this.messages = [];
    page.on("console", (message) => {
      this.messages.push({ type: message.type(), text: message.text() });
    });
    page.on("pageerror", (error) => {
      this.messages.push({ type: "pageerror", text: error.message });
    });
  }

  async navigate(url, allowStartupError = false, waitForSummary = false) {
    await this.page.goto(url, { waitUntil: "load" });
    await this.waitUntilReady(allowStartupError, waitForSummary);
  }

  async reload(waitForSummary = false) {
    const marker = String(++this.reloadGeneration);
    await this.page.evaluate((value) => {
      document.documentElement.dataset.gwrBrowserReload = value;
    }, marker);
    await this.page.reload({ waitUntil: "load" });
    await this.waitForCondition(
      `document.documentElement.dataset.gwrBrowserReload !== ${JSON.stringify(marker)}`,
      "the reloaded document to replace the previous page",
    );
    await this.waitUntilReady(false, waitForSummary);
  }

  async waitUntilReady(allowStartupError = false, waitForSummary = false) {
    const handle = await this.page.waitForFunction(
      reportStatus,
      waitForSummary,
      {
        timeout: 120_000,
      },
    );
    const status = await handle.jsonValue();
    await handle.dispose();
    throwReportError("Chromium", status, allowStartupError);
  }

  async coldStartup() {
    return this.page.evaluate(
      () =>
        performance.getEntriesByName("gwr-initial-summary-ready").at(-1)
          ?.startTime,
    );
  }

  async measureAction(source) {
    return this.page.evaluate(async (action) => {
      const start = performance.now();
      Function(action)();
      window.GWR_BENCHMARK_FLUSH?.();
      await afterCurrentTask();
      document.body.offsetHeight;
      return performance.now() - start;

      function afterCurrentTask() {
        return new Promise((resolve) => setTimeout(resolve, 0));
      }
    }, source);
  }

  async measureKernel(name, iterations) {
    return this.page.evaluate(
      ({ name, iterations }) => {
        const run = wasm_bindgen.benchmark_kernel;
        const start = performance.now();
        const checksum = run(name, iterations);
        return { milliseconds: performance.now() - start, checksum };
      },
      { name, iterations },
    );
  }

  async evaluate(source) {
    return this.page.evaluate(
      (script) => Function(`return (${script})`)(),
      source,
    );
  }

  async waitForCondition(condition, description, timeout = 120_000) {
    try {
      const handle = await this.page.waitForFunction(
        (source) => Function(`return (${source})`)(),
        condition,
        { timeout },
      );
      await handle.dispose();
    } catch (error) {
      throw conditionWaitError(description, condition, error);
    }
  }

  async screenshot(destination) {
    await this.page.screenshot({ path: destination, fullPage: true });
  }

  async pageState() {
    return this.page.evaluate(reportPageState);
  }

  async pageSource() {
    return this.page.content();
  }

  async consoleMessages() {
    return this.messages;
  }
}

class SafariAdapter {
  constructor() {
    this.name = "safari";
    this.version = null;
  }

  async withSession(
    url,
    action,
    {
      allowStartupError = false,
      waitForSummary = false,
      failureEvidence = null,
    } = {},
  ) {
    const report = await serveReport(url);
    try {
      const driver = await createSafariDriver();
      try {
        await driver.manage().setTimeouts({
          pageLoad: 120_000,
          script: 120_000,
        });
        await driver.manage().window().setRect({ width: 1440, height: 1000 });
        const capabilities = await driver.getCapabilities();
        this.version ||= capabilities.get("browserVersion") || "unknown";
        const session = new SafariSession(driver);
        try {
          await session.navigate(report.url, allowStartupError, waitForSummary);
          return await action(session);
        } catch (error) {
          if (failureEvidence) {
            await captureFailure(session, failureEvidence);
          }
          throw error;
        }
      } finally {
        await driver.quit();
      }
    } finally {
      await report.close();
    }
  }
}

class SafariSession {
  constructor(driver) {
    this.driver = driver;
    this.reloadGeneration = 0;
  }

  async navigate(url, allowStartupError = false, waitForSummary = false) {
    await this.driver.get(url);
    await this.waitUntilReady(allowStartupError, waitForSummary);
  }

  async reload(waitForSummary = false) {
    const marker = String(++this.reloadGeneration);
    await this.driver.executeScript(
      "document.documentElement.dataset.gwrBrowserReload = arguments[0]",
      marker,
    );
    await this.driver.navigate().refresh();
    await this.waitForCondition(
      `document.documentElement.dataset.gwrBrowserReload !== ${JSON.stringify(marker)}`,
      "the reloaded document to replace the previous page",
    );
    await this.waitUntilReady(false, waitForSummary);
  }

  async waitUntilReady(allowStartupError = false, waitForSummary = false) {
    let status;
    try {
      status = await this.driver.wait(
        () => this.driver.executeScript(reportStatus, waitForSummary),
        120_000,
      );
    } catch (error) {
      const state = await this.driver.executeScript(reportPageState);
      throw new Error(
        `${error.message}; Safari report state: ${JSON.stringify(state)}`,
        { cause: error },
      );
    }
    throwReportError("Safari", status, allowStartupError);
  }

  async coldStartup() {
    return this.driver.executeScript(
      "return performance.getEntriesByName('gwr-initial-summary-ready').at(-1)?.startTime",
    );
  }

  async measureAction(source) {
    return this.driver.executeAsyncScript(
      `const source = arguments[0];
       const done = arguments[arguments.length - 1];
       const start = performance.now();
       Function(source)();
       window.GWR_BENCHMARK_FLUSH?.();
       setTimeout(() => {
         document.body.offsetHeight;
         done(performance.now() - start);
       }, 0);`,
      source,
    );
  }

  async measureKernel(name, iterations) {
    return this.driver.executeScript(
      `const run = wasm_bindgen.benchmark_kernel;
       const start = performance.now();
       const checksum = run(arguments[0], arguments[1]);
       return { milliseconds: performance.now() - start, checksum };`,
      name,
      iterations,
    );
  }

  async evaluate(source) {
    return this.driver.executeScript(`return (${source})`);
  }

  async waitForCondition(condition, description, timeout = 120_000) {
    try {
      await this.driver.wait(
        () =>
          this.driver.executeScript(
            "return Boolean(Function('return (' + arguments[0] + ')')());",
            condition,
          ),
        timeout,
      );
    } catch (error) {
      throw conditionWaitError(description, condition, error);
    }
  }

  async screenshot(destination) {
    await writeFile(destination, await this.driver.takeScreenshot(), "base64");
  }

  async pageState() {
    return this.driver.executeScript(reportPageState);
  }

  async pageSource() {
    return this.driver.getPageSource();
  }

  async consoleMessages() {
    try {
      return await this.driver.manage().logs().get("browser");
    } catch (error) {
      return [{ type: "unavailable", text: error.message }];
    }
  }
}

async function createSafariDriver() {
  try {
    const options = new safari.Options().setPageLoadStrategy("none");
    return await new Builder()
      .forBrowser("safari")
      .setSafariOptions(options)
      .build();
  } catch (error) {
    throw new Error(
      `Unable to start safaridriver. Enable Safari > Develop > Allow Remote Automation. ${error}`,
    );
  }
}

function reportStatus(waitForSummary = false) {
  const root = document.documentElement;
  const error = root.dataset.gwrError || null;
  const marker = waitForSummary ? "gwrSummaryReady" : "gwrReady";
  if (root.dataset[marker] !== "complete" && !error) {
    return null;
  }
  return { error };
}

function reportPageState() {
  const root = document.documentElement;
  return {
    url: location.href,
    readyState: document.readyState,
    reportSummaryReady: root.dataset.gwrSummaryReady || null,
    reportReady: root.dataset.gwrReady || null,
    reportError: root.dataset.gwrError || null,
    scripts: Array.from(document.scripts, (script) => script.src || "inline"),
  };
}

function throwReportError(browser, status, allowStartupError = false) {
  if (status.error && !allowStartupError) {
    throw new Error(`${browser} report startup failed: ${status.error}`);
  }
}

function conditionWaitError(description, condition, error) {
  return new Error(
    `Failed while waiting for ${description}; unmet condition: ${condition}. ${error.message}`,
    { cause: error },
  );
}

async function captureFailure(session, { directory, name }) {
  await mkdir(directory, { recursive: true });
  const errors = [];
  const evidence = { page: null, console: [] };
  try {
    evidence.page = await session.pageState();
  } catch (error) {
    errors.push(`page state: ${error.message}`);
  }
  try {
    evidence.console = await session.consoleMessages();
  } catch (error) {
    errors.push(`console: ${error.message}`);
  }
  try {
    await writeFile(
      path.join(directory, `${name}.html`),
      await session.pageSource(),
    );
  } catch (error) {
    errors.push(`DOM: ${error.message}`);
  }
  try {
    await session.screenshot(path.join(directory, `${name}.png`));
  } catch (error) {
    errors.push(`screenshot: ${error.message}`);
  }
  evidence.captureErrors = errors;
  await writeFile(
    path.join(directory, `${name}.json`),
    `${JSON.stringify(evidence, null, 2)}\n`,
  );
}
