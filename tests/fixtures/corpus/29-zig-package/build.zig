const std = @import("std");

pub fn build(b: *std.Build) void {
    _ = b.step("check", "Check the project");
}
