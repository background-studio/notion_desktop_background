#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("创建 Notion worker 运行时失败");
    runtime.block_on(notion_background_studio_lib::run());
}
