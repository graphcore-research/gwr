// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

#[cfg(test)]
mod tests {
    // Note that all tests need to be run serially as spotter uses a global state
    // shared between threads.

    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};

    use serial_test::serial;

    use crate::app::{App, InputState};
    use crate::filter::Filter;
    use crate::renderer::Renderer;
    use crate::rocket::SHARED_STATE;

    const FRAME_HEIGHT: usize = 30;
    const NUM_RENDER_LINES: usize = 200;

    /// Create a `Renderer` with initial state that allows us to test movement.
    fn create_test_renderer() -> Renderer {
        let mut renderer = Renderer::new();
        // Set a reasonable frame size so block moves are non-zero.
        renderer.set_frame_size(FRAME_HEIGHT);

        // Pretend we have renderable lines to allow movement
        renderer.num_render_lines = NUM_RENDER_LINES;
        renderer
    }

    /// Create a `Filter` with a channel that will accept everything without
    /// blocking. Don't bother to start a background filter thread as we
    /// don't test the actual line content.
    ///
    /// Note: Need to return receiver so it isn't dropped - otherwise send()
    /// will fail.
    fn create_test_filter() -> (Filter, Receiver<()>) {
        let (tx, rx) = channel();
        (Filter::new(tx), rx)
    }

    /// Build an `App` wired to our test Renderer and Filter.
    fn create_test_app_with(renderer: Renderer, filter: Filter) -> App {
        App {
            running: true,
            renderer: Arc::new(Mutex::new(renderer)),
            filter: Arc::new(Mutex::new(filter)),
            input_state: InputState::Default,
            numbers: String::new(),
        }
    }

    fn create_test_app() -> (App, Receiver<()>) {
        {
            // Reset shared global state
            let mut guard = SHARED_STATE.lock().unwrap();
            guard.reset();
        }

        let renderer = create_test_renderer();
        let (filter, rx) = create_test_filter();
        (create_test_app_with(renderer, filter), rx)
    }

    #[test]
    #[serial]
    fn tick_consumes_command_and_updates_filter() {
        let (mut app, _rx) = create_test_app();

        // Put a command into the shared state.
        {
            let mut guard = SHARED_STATE.lock().unwrap();
            guard.command = Some("foo bar".to_string());
        }

        // Move somewhere away from the top so we can observe move_top() happens.
        {
            let mut r = app.renderer.lock().unwrap();
            // Move down 10 lines so we’re at line 11.
            r.move_down_lines(10);
            assert_eq!(r.current_render_line_number(), 11);
        }

        app.tick();

        {
            let guard = SHARED_STATE.lock().unwrap();
            assert!(
                guard.command.is_none(),
                "tick() should consume SHARED_STATE.command"
            );
        }

        {
            let f = app.filter.lock().unwrap();
            assert_eq!(
                f.search, "foo bar",
                "tick() should pass the command string to Filter::set"
            );
        }

        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                1,
                "tick() should move the view to the top after setting the filter"
            );
        }
    }

    #[test]
    #[serial]
    fn tick_does_nothing_when_no_command() {
        let (mut app, _rx) = create_test_app();

        // Ensure no command is present.
        {
            let mut guard = SHARED_STATE.lock().unwrap();
            guard.command = None;
        }

        // Setup some initial filter state.
        {
            let mut f = app.filter.lock().unwrap();
            f.search = "keep-me".to_string();
        }

        // And initial renderer state.
        {
            let mut r = app.renderer.lock().unwrap();
            r.move_down_lines(5);
            assert_eq!(r.current_render_line_number(), 6);
        }

        app.tick();

        {
            let f = app.filter.lock().unwrap();
            assert_eq!(
                f.search, "keep-me",
                "tick() should not touch the filter when there is no command"
            );
        }

        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                6,
                "tick() should not move the view when there is no command"
            );
        }
    }

    #[test]
    #[serial]
    fn quit_sets_running_false() {
        let (mut app, _rx) = create_test_app();
        assert!(app.running);
        app.quit();
        assert!(!app.running);
    }

    #[test]
    #[serial]
    fn app_starts_on_line_1() {
        // Lots of tests assume that we start at line 1, so check it here.
        let (app, _rx) = create_test_app();
        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                1,
                "app should start on line 1"
            );
        }
    }

    #[test]
    #[serial]
    fn move_top_and_bottom_delegate_to_renderer() {
        let (mut app, _rx) = create_test_app();

        {
            let mut r = app.renderer.lock().unwrap();
            // Move near the bottom first.
            r.move_down_lines(150);
            assert!(r.current_render_line_number() > 1);
        }

        app.move_top();
        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                1,
                "move_top() should move to the very first line"
            );
        }

        app.move_bottom();
        {
            let r = app.renderer.lock().unwrap();
            // Will move near to the end, but not quite there.
            assert!(
                r.current_render_line_number() > (NUM_RENDER_LINES * 3 / 4),
                "move_bottom() should move away from the top when there are many lines"
            );
        }
    }

    #[test]
    #[serial]
    fn move_up_and_down_lines_delegate_to_renderer() {
        let (mut app, _rx) = create_test_app();

        app.move_down_lines(5);
        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                6,
                "move_down_lines(5) should move 5 lines down"
            );
        }

        app.move_up_lines(3);
        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                3,
                "move_up_lines(3) should move 3 lines up"
            );
        }
    }

    #[test]
    #[serial]
    fn move_up_and_down_block_use_block_move_lines() {
        let (mut app, _rx) = create_test_app();

        // Expect block moves to move 1/3 of the frame.
        let expect_block_move_lines = FRAME_HEIGHT / 3;
        let initial_move_lines = NUM_RENDER_LINES / 4;

        {
            let mut r = app.renderer.lock().unwrap();

            // Move to a point somewhere in the middle.
            r.move_down_lines(initial_move_lines);

            assert_eq!(r.block_move_lines, expect_block_move_lines);
        }

        app.move_up_block();
        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                1 + initial_move_lines - expect_block_move_lines,
                "move_up_block() should move up by block_move_lines"
            );
        }

        app.move_down_block();
        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                1 + initial_move_lines,
                "move_down_block() should move down by block_move_lines"
            );
        }
    }

    #[test]
    #[serial]
    fn move_to_number_uses_numbers_buffer() {
        let (mut app, _rx) = create_test_app();
        // Pretend the user typed "42".
        app.numbers = "42".to_string();

        app.move_to_number();

        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                42,
                "move_to_number() should jump to the requested line number"
            );
        }

        assert!(
            app.numbers.is_empty(),
            "move_to_number() should clear the numbers buffer"
        );
    }

    #[test]
    #[serial]
    fn move_down_n_uses_numbers_buffer() {
        let (mut app, _rx) = create_test_app();
        // Pretend the user typed "7".
        app.numbers = "7".to_string();

        app.move_down_n();

        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                8,
                "move_down_n() should move down by the parsed number of lines"
            );
        }

        assert!(
            app.numbers.is_empty(),
            "move_down_n() should clear the numbers buffer"
        );
    }

    #[test]
    #[serial]
    fn move_to_percent_uses_numbers_buffer() {
        let (mut app, _rx) = create_test_app();

        // Move to 50% of render lines.
        app.numbers = "50".to_string();
        app.move_to_percent();

        {
            let r = app.renderer.lock().unwrap();
            assert_eq!(
                r.current_render_line_number(),
                NUM_RENDER_LINES / 2,
                "move_to_percent() should jump to the correct percentage of the file"
            );
        }

        assert!(
            app.numbers.is_empty(),
            "move_to_percent() should clear the numbers buffer"
        );
    }

    #[test]
    #[serial]
    fn state_getter_and_setter_work() {
        let (mut app, _rx) = create_test_app();
        assert_eq!(app.state(), InputState::Default);

        app.set_state(InputState::Search);
        assert_eq!(app.state(), InputState::Search);

        app.set_state(InputState::Numbers);
        assert_eq!(app.state(), InputState::Numbers);
    }

    #[test]
    #[serial]
    fn toggles_flip_renderer_flags() {
        let (mut app, _rx) = create_test_app();

        {
            let r = app.renderer.lock().unwrap();
            assert!(!r.plot_fullness);
            assert!(r.print_names);
            assert!(!r.print_packets);
            assert!(r.print_times);
        }

        app.toggle_plot_fullness();
        app.toggle_print_names();
        app.toggle_print_packets();
        app.toggle_print_times();

        {
            let r = app.renderer.lock().unwrap();
            assert!(r.plot_fullness, "plot_fullness should be toggled on");
            assert!(!r.print_names, "print_names should be toggled off");
            assert!(r.print_packets, "print_packets should be toggled on");
            assert!(!r.print_times, "print_times should be toggled off");
        }
    }

    #[test]
    #[serial]
    fn number_buffer_helpers_work() {
        let (mut app, _rx) = create_test_app();

        app.push_number_char('1');
        app.push_number_char('2');
        app.push_number_char('3');
        assert_eq!(app.numbers, "123");

        app.pop_number_char();
        assert_eq!(app.numbers, "12");

        app.clear_numbers();
        assert!(app.numbers.is_empty());
    }
}
