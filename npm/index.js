#!/usr/bin/env node
const { spawn } = require("child_process");
const path = require("path");

const exe = path.join(__dirname, "bin", "Sweep.exe");

spawn(exe, process.argv.slice(2), {
    stdio: "inherit"
});
