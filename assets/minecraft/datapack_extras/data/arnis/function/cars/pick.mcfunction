# The next lane, joined at the first of its points ahead of the car, and whether getting
# there crosses a junction.
$data modify entity @s data merge from storage arnis:cars seg[$(s)].n[$(k)]
function arnis:cars/enter with entity @s data
