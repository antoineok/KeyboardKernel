{
  lib,
  buildPythonApplication,
  fetchFromGitHub,
  setuptools,
  flask,
  flask-socketio,
  pygments,
  eventlet,
  python-socketio,
}:
# gdb-frontend from pip
buildPythonApplication rec {
  pname = "gdbfrontend";
  version = "unstable-2025-01-11";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "rohanrhu";
    repo = "gdb-frontend";
    rev = "4daf786e4fdcce57348844f09c9979804ccc01c4";
    hash = "sha256-tK+0O8ImeONQjR2vd0WI1jMAZDgfstIJpRBBjWFq0OI=";
  };

  build-system = [setuptools];

  propagatedBuildInputs = [
    flask
    flask-socketio
    pygments
    eventlet
    python-socketio
  ];

  doCheck = false;

  meta = with lib; {
    description = "An easy, flexible, and extensible GUI debugger frontend for GDB.";
    homepage = "https://github.com/rohanrhu/gdb-frontend";
    license = licenses.gpl3Plus;
  };
}
