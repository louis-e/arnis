# Experimental Arnis Rust Development Branch
This branch is dedicated to the ongoing effort to port the Arnis project from Python to Rust.

Run with the following arguments: ```cargo run --release -- --path "C:/YOUR_PATH/.minecraft/saves/worldname" --bbox "11.885287,48.292645,11.892861,48.295757"```
The bbox parameter is only required because the input argument parsing is already ported, but there's no processing logic implemented behind it yet.

Key objectives of this port:
- **Modularity**: Ensure that all components (e.g., data fetching, processing, and world generation) are cleanly separated into distinct modules for better maintainability and scalability.
- **Cross-Platform Support**: Ensure the project runs smoothly on Windows, macOS, and Linux.
- **Comprehensive Documentation**: Detailed in-code documentation for a clear structure and logic.
- **User-Friendly Experience**: Focus on making the project easy to use for end users, with the potential to develop a graphical user interface (GUI) in the future. Suggestions and discussions on UI/UX are welcome.
- **Performance Optimization**: Utilize Rust’s memory safety and concurrency features to optimize the performance of the world generation process.


Currently only some very basic functionalities from the main branch to later build on are implemented here. Please feel free to take on any unported logic, propose improvements, or suggest UI/UX enhancements. Contributions, discussions and suggestions are more than welcome.

Let's work together to build a high-performance, cross-platform, and user-friendly Rust version of Arnis!
