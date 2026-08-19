// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

/* global wasm_bindgen */

import { writeFile } from "node:fs/promises";
import os from "node:os";

import { chromium } from "playwright-core";
import webdriver from "selenium-webdriver";
import safari from "selenium-webdriver/safari.js";

const { Builder, By, until } = webdriver;

export function browserAdapters(config) {
  return config.browsers.map((name) => {
    if (name === "chromium") {
      return new ChromiumAdapter(config.chromiumExecutable);
    }
    if (name === "safari") {
      if (os.platform() !== "darwin") {
        throw new Error("Safari benchmarks require macOS and the system safaridriver");
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

  async withSession(url, action) {
    const browser = await chromium.launch({
      executablePath: this.executable,
      headless: true,
      args: ["--allow-file-access-from-files"],
    });
    this.version ||= browser.version();
    try {
      const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
      const session = new ChromiumSession(page);
      await session.navigate(url);
      return await action(session);
    } finally {
      await browser.close();
    }
  }
}

class ChromiumSession {
  constructor(page) {
    this.page = page;
  }

  async navigate(url) {
    await this.page.goto(url, { waitUntil: "load" });
    await this.page.waitForFunction(
      () =>
        document.documentElement.dataset.gwrReady === "complete" ||
        document.documentElement.dataset.gwrError,
      null,
      { timeout: 120_000 },
    );
    await this.throwStartupError();
  }

  async reload() {
    await this.page.reload({ waitUntil: "load" });
    await this.page.waitForFunction(
      () => document.documentElement.dataset.gwrReady === "complete",
      null,
      { timeout: 120_000 },
    );
  }

  async coldStartup() {
    return this.page.evaluate(() =>
      performance.getEntriesByName("gwr-initial-summary-ready").at(-1)?.startTime,
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
        const run = window.GWR_BENCHMARK_KERNELS?.run || wasm_bindgen.benchmark_kernel;
        const start = performance.now();
        const checksum = run(name, iterations);
        return { milliseconds: performance.now() - start, checksum };
      },
      { name, iterations },
    );
  }

  async evaluate(source) {
    return this.page.evaluate((script) => Function(`return (${script})`)(), source);
  }

  async wait(milliseconds) {
    await this.page.waitForTimeout(milliseconds);
  }

  async screenshot(destination) {
    await this.page.screenshot({ path: destination, fullPage: true });
  }

  async throwStartupError() {
    const error = await this.page.evaluate(
      () => document.documentElement.dataset.gwrError || null,
    );
    if (error) {
      throw new Error(`Chromium report startup failed: ${error}`);
    }
  }
}

class SafariAdapter {
  constructor() {
    this.name = "safari";
    this.version = null;
  }

  async withSession(url, action) {
    let driver;
    try {
      driver = await new Builder()
        .forBrowser("safari")
        .setSafariOptions(new safari.Options())
        .build();
    } catch (error) {
      throw new Error(
        `Unable to start safaridriver. Enable Safari > Develop > Allow Remote Automation. ${error}`,
      );
    }
    try {
      await driver.manage().setTimeouts({ pageLoad: 120_000, script: 120_000 });
      await driver.manage().window().setRect({ width: 1440, height: 1000 });
      const capabilities = await driver.getCapabilities();
      this.version ||= capabilities.get("browserVersion") || "unknown";
      const session = new SafariSession(driver);
      await session.navigate(url);
      return await action(session);
    } finally {
      await driver.quit();
    }
  }
}

class SafariSession {
  constructor(driver) {
    this.driver = driver;
  }

  async navigate(url) {
    await this.driver.get(url);
    await this.driver.wait(
      until.elementLocated(By.css("html[data-gwr-ready='complete'], html[data-gwr-error]")),
      120_000,
    );
    const error = await this.driver.executeScript(
      "return document.documentElement.dataset.gwrError || null",
    );
    if (error) {
      throw new Error(`Safari report startup failed: ${error}`);
    }
  }

  async reload() {
    await this.driver.navigate().refresh();
    await this.driver.wait(
      until.elementLocated(By.css("html[data-gwr-ready='complete']")),
      120_000,
    );
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
      `const run = window.GWR_BENCHMARK_KERNELS?.run || wasm_bindgen.benchmark_kernel;
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

  async wait(milliseconds) {
    await this.driver.sleep(milliseconds);
  }

  async screenshot(destination) {
    await writeFile(destination, await this.driver.takeScreenshot(), "base64");
  }
}
