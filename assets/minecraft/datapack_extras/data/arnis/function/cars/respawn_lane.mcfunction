# Not on a lane no car starts on, as a stub or a scrap of road with nowhere to go.
$execute if data storage arnis:cars seg[$(s)].o run return 0
$data modify storage arnis:cars spawn merge from storage arnis:cars seg[$(s)].p[0]
function arnis:cars/respawn_at with storage arnis:cars spawn
