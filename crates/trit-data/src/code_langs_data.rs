//! 编程语言特征表（前后端 / GPU / 着色器 / 定理证明 / 命令行方言）。

pub const PROBES: &[(&str, &[&str])] = &[
        (
            "python",
            &["def ", "import ", "print(", "self.", "__init__", "elif "],
        ),
        (
            "javascript",
            &[
                "function ",
                "=>",
                "const ",
                "console.log",
                "require(",
                "document.",
            ],
        ),
        (
            "typescript",
            &["interface ", ": string", ": number", "enum ", "as const"],
        ),
        (
            "rust",
            &["fn ", "let mut", "impl ", "trait ", "cargo ", "-> "],
        ),
        (
            "java",
            &[
                "public class",
                "system.out",
                "import java",
                "@override",
                "void ",
            ],
        ),
        (
            "c",
            &["#include", "int main", "printf(", "malloc(", "stdio.h"],
        ),
        (
            "cpp",
            &["std::", "template<", "cout <<", "namespace ", "vector<"],
        ),
        (
            "csharp",
            &[
                "using system",
                "namespace ",
                "console.writeline",
                "public class",
            ],
        ),
        ("go", &["package ", "func ", "fmt.", ":="]),
        ("php", &["<?php", "echo ", "$this", "function "]),
        ("ruby", &["def ", "puts ", "require '", "end"]),
        (
            "sql",
            &["select ", "from ", "where ", "join ", "insert into"],
        ),
        ("html", &["<html", "<div", "</", "<!doctype"]),
        ("css", &["color:", "margin:", "@media", "display:"]),
        (
            "shell",
            &["#!/", "echo ", "export ", "sudo ", "apt ", "npm ", "pip "],
        ),
        ("kotlin", &["fun ", "val ", "when ("]),
        ("swift", &["func ", "guard ", "import foundation"]),
        ("scala", &["case class", "object ", "def "]),
        ("vue", &["<template>", "v-if", "v-for"]),
        (
            "glsl",
            &[
                "#version",
                "gl_fragcolor",
                "uniform ",
                "varying ",
                "vec4 ",
                "mat4 ",
            ],
        ),
        (
            "hlsl",
            &[
                "cbuffer ",
                "sv_position",
                "float4 ",
                "texture2d",
                "#pragma pack_matrix",
            ],
        ),
        (
            "wgsl",
            &[
                "@vertex",
                "@fragment",
                "@compute",
                "var<uniform>",
                "@group(",
            ],
        ),
        (
            "cuda",
            &[
                "__global__",
                "__device__",
                "__shared__",
                "cudamalloc",
                "<<<",
            ],
        ),
        ("opencl", &["__kernel", "get_global_id", "__local"]),
        (
            "metal",
            &[
                "#include <metal_stdlib>",
                "kernel void",
                "metal::",
                "[[buffer(",
            ],
        ),
        ("spirv", &["optypeint", "opfunction", "spir-v", "%1 = op"]),
        (
            "lean",
            &[
                "theorem ",
                "lemma ",
                "inductive ",
                "import mathlib",
                "by ",
                "∀",
                "∃",
            ],
        ),
        ("assembly", &["mov ", "push ", "pop ", "section .text"]),
        (
            "verilog",
            &["module ", "always @", "endmodule", "wire ", "reg "],
        ),
        ("solidity", &["pragma solidity", "contract ", "msg.sender"]),
        ("dart", &["import 'package:", "void main()", "widget "]),
        ("haskell", &["module ", "where", "::", "data ", "instance "]),
        ("lua", &["local ", "function ", "end"]),
        ("julia", &["function ", "end", "using "]),
        ("r", &["<-", "function(", "library("]),
        ("bash", &["#!/bin/bash", "#!/usr/bin/env bash", "[[ ", "$("]),
        ("zsh", &["#!/bin/zsh", "setopt ", "zstyle ", "autoload "]),
        ("fish", &["#!/usr/bin/fish", "set -x", " end"]),
        ("powershell", &["param(", "write-host", "$env:", "get-childitem", "pwsh"]),
        ("cmd", &["@echo off", "setlocal", "%errorlevel%", "goto "]),
        ("nushell", &["| where", "| each", "def "]),
    ];
